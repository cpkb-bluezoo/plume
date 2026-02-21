/*
 * crypto.rs
 * Copyright (C) 2026 Chris Burdess
 *
 * This file is part of Plume, a Nostr desktop client.
 *
 * Plume is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * Plume is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with Plume.  If not, see <http://www.gnu.org/licenses/>.
 */

// Nostr uses secp256k1 Schnorr signatures (BIP-340)
// Event IDs are SHA256 hashes of the serialized event

use secp256k1::ecdh::shared_secret_point;
use secp256k1::{schnorr, Keypair, Parity, PublicKey, Secp256k1, SecretKey, XOnlyPublicKey};
use sha2::{Digest, Sha256};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use crate::nostr::{
    Event, KIND_BLOSSOM_AUTH, KIND_CHAT_MESSAGE, KIND_DM, KIND_GIFT_WRAP, KIND_HTTP_AUTH,
    KIND_SEAL, KIND_ZAP_REQUEST, event_to_json_compact, parse_event,
};

use aes::cipher::block_padding::Pkcs7;
use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use cbc::{Decryptor, Encryptor};
type Aes256CbcEnc = Encryptor<aes::Aes256>;
type Aes256CbcDec = Decryptor<aes::Aes256>;

use chacha20::cipher::StreamCipher;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};

// ============================================================
// Event ID Computation
// ============================================================

// Compute the event ID (SHA256 hash of serialized event)
// The event is serialized as: [0, pubkey, created_at, kind, tags, content]
// See: https://github.com/nostr-protocol/nips/blob/master/01.md
pub fn compute_event_id(event: &Event) -> Result<String, String> {
    // Serialize the event for hashing
    let serialized = serialize_event_for_id(event)?;
    
    // Compute SHA256 hash
    let hash = sha256_hash(serialized.as_bytes());
    
    // Convert to hex string
    let hex_id = bytes_to_hex(&hash);
    
    return Ok(hex_id);
}

// Serialize an event for ID computation
// Format: [0, pubkey, created_at, kind, tags, content]
fn serialize_event_for_id(event: &Event) -> Result<String, String> {
    let mut json = String::new();
    
    // Start array
    json.push_str("[0,\"");
    
    // pubkey (must be lowercase hex)
    json.push_str(&event.pubkey.to_lowercase());
    json.push_str("\",");
    
    // created_at (integer)
    json.push_str(&event.created_at.to_string());
    json.push_str(",");
    
    // kind (integer)
    json.push_str(&event.kind.to_string());
    json.push_str(",");
    
    // tags (array of arrays)
    json.push_str("[");
    for (i, tag) in event.tags.iter().enumerate() {
        json.push_str("[");
        for (j, item) in tag.iter().enumerate() {
            json.push_str("\"");
            json.push_str(&escape_json_string(item));
            json.push_str("\"");
            if j < tag.len() - 1 {
                json.push_str(",");
            }
        }
        json.push_str("]");
        if i < event.tags.len() - 1 {
            json.push_str(",");
        }
    }
    json.push_str("],\"");
    
    // content (string)
    json.push_str(&escape_json_string(&event.content));
    json.push_str("\"]");
    
    return Ok(json);
}

// ============================================================
// Signature Verification
// ============================================================

// Verify an event's signature
// Returns true if the signature is valid, false otherwise
pub fn verify_event_signature(event: &Event) -> Result<bool, String> {
    // Get the secp256k1 context
    let secp = Secp256k1::verification_only();
    
    // Parse the public key (x-only format for Schnorr)
    let pubkey_bytes = match hex_to_bytes(&event.pubkey) {
        Ok(bytes) => bytes,
        Err(e) => return Err(format!("Invalid pubkey hex: {}", e)),
    };
    
    if pubkey_bytes.len() != 32 {
        return Err(format!("Invalid pubkey length: expected 32 bytes, got {}", pubkey_bytes.len()));
    }
    
    let xonly_pubkey = match XOnlyPublicKey::from_slice(&pubkey_bytes) {
        Ok(pk) => pk,
        Err(e) => return Err(format!("Invalid public key: {}", e)),
    };
    
    // Parse the signature (64 bytes)
    let sig_bytes = match hex_to_bytes(&event.sig) {
        Ok(bytes) => bytes,
        Err(e) => return Err(format!("Invalid signature hex: {}", e)),
    };
    
    if sig_bytes.len() != 64 {
        return Err(format!("Invalid signature length: expected 64 bytes, got {}", sig_bytes.len()));
    }
    
    let signature = match schnorr::Signature::from_slice(&sig_bytes) {
        Ok(sig) => sig,
        Err(e) => return Err(format!("Invalid signature format: {}", e)),
    };
    
    // Compute the message hash (event ID)
    let serialized = serialize_event_for_id(event)?;
    let message_hash = sha256_hash(serialized.as_bytes());
    
    // Create a message from the hash
    let message = match secp256k1::Message::from_digest_slice(&message_hash) {
        Ok(msg) => msg,
        Err(e) => return Err(format!("Failed to create message: {}", e)),
    };
    
    // Verify the signature
    match secp.verify_schnorr(&signature, &message, &xonly_pubkey) {
        Ok(()) => return Ok(true),
        Err(_) => return Ok(false),
    }
}

// Verify that an event's ID matches its content
pub fn verify_event_id(event: &Event) -> Result<bool, String> {
    let computed_id = compute_event_id(event)?;
    
    // Compare (case-insensitive)
    let id_matches = computed_id.to_lowercase() == event.id.to_lowercase();
    
    return Ok(id_matches);
}

// Fully verify an event (ID and signature)
pub fn verify_event(event: &Event) -> Result<VerificationResult, String> {
    // First check the ID
    let id_valid = verify_event_id(event)?;
    if !id_valid {
        return Ok(VerificationResult {
            valid: false,
            id_valid: false,
            signature_valid: false,
            error: Some(String::from("Event ID does not match content")),
        });
    }
    
    // Then check the signature
    let sig_valid = verify_event_signature(event)?;
    if !sig_valid {
        return Ok(VerificationResult {
            valid: false,
            id_valid: true,
            signature_valid: false,
            error: Some(String::from("Signature verification failed")),
        });
    }
    
    // Both valid
    return Ok(VerificationResult {
        valid: true,
        id_valid: true,
        signature_valid: true,
        error: None,
    });
}

// Result of event verification
pub struct VerificationResult {
    pub valid: bool,
    pub id_valid: bool,
    pub signature_valid: bool,
    pub error: Option<String>,
}

// Convert VerificationResult to JSON
pub fn verification_result_to_json(result: &VerificationResult) -> String {
    let mut json = String::new();
    json.push_str("{");
    
    json.push_str("\"valid\":");
    json.push_str(if result.valid { "true" } else { "false" });
    
    json.push_str(",\"id_valid\":");
    json.push_str(if result.id_valid { "true" } else { "false" });
    
    json.push_str(",\"signature_valid\":");
    json.push_str(if result.signature_valid { "true" } else { "false" });
    
    if let Some(ref error) = result.error {
        json.push_str(",\"error\":\"");
        json.push_str(&escape_json_string(error));
        json.push_str("\"");
    }
    
    json.push_str("}");
    return json;
}

// ============================================================
// Key Generation
// ============================================================

// Generate a new random key pair
// Returns (secret_key_hex, public_key_hex)
pub fn generate_keypair() -> Result<(String, String), String> {
    use std::time::{SystemTime, UNIX_EPOCH};
    
    // Get some entropy from system time and random-ish sources
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    
    // Create a seed from multiple sources
    let mut seed = [0u8; 32];
    
    // Mix in nanoseconds
    let nanos = now.as_nanos();
    for i in 0..16 {
        seed[i] = ((nanos >> (i * 8)) & 0xff) as u8;
    }
    
    // Mix in process ID and thread ID for more entropy
    let pid = std::process::id();
    seed[16] = (pid & 0xff) as u8;
    seed[17] = ((pid >> 8) & 0xff) as u8;
    seed[18] = ((pid >> 16) & 0xff) as u8;
    seed[19] = ((pid >> 24) & 0xff) as u8;
    
    // Mix in some memory address randomness
    let stack_addr = &seed as *const _ as usize;
    for i in 0..8 {
        seed[20 + i] = ((stack_addr >> (i * 8)) & 0xff) as u8;
    }
    
    // Hash the seed to get uniform randomness
    let mut hasher = Sha256::new();
    hasher.update(&seed);
    let hash_result = hasher.finalize();
    
    let mut secret_bytes = [0u8; 32];
    secret_bytes.copy_from_slice(&hash_result);
    
    // Create the secret key
    let secret_key = match SecretKey::from_slice(&secret_bytes) {
        Ok(sk) => sk,
        Err(e) => return Err(format!("Failed to create secret key: {}", e)),
    };
    
    // Derive the public key
    let secp = Secp256k1::new();
    let keypair = Keypair::from_secret_key(&secp, &secret_key);
    let (xonly_pubkey, _parity) = XOnlyPublicKey::from_keypair(&keypair);
    
    // Convert to hex
    let secret_hex = bytes_to_hex(&secret_bytes);
    let pubkey_hex = bytes_to_hex(&xonly_pubkey.serialize());
    
    return Ok((secret_hex, pubkey_hex));
}

// ============================================================
// Event Signing
// ============================================================

// Get the public key (x-only, 32 bytes) from a secret key
pub fn get_public_key_from_secret(secret_key_hex: &str) -> Result<String, String> {
    // Parse the secret key
    let secret_bytes = hex_to_bytes(secret_key_hex)?;
    
    if secret_bytes.len() != 32 {
        return Err(format!("Invalid secret key length: expected 32 bytes, got {}", secret_bytes.len()));
    }
    
    let secret_key = match SecretKey::from_slice(&secret_bytes) {
        Ok(sk) => sk,
        Err(e) => return Err(format!("Invalid secret key: {}", e)),
    };
    
    // Create keypair and get x-only public key
    let secp = Secp256k1::new();
    let keypair = Keypair::from_secret_key(&secp, &secret_key);
    let (xonly_pubkey, _parity) = XOnlyPublicKey::from_keypair(&keypair);
    
    // Convert to hex
    let pubkey_hex = bytes_to_hex(&xonly_pubkey.serialize());
    
    return Ok(pubkey_hex);
}

// Sign an event with a secret key
// The event should have pubkey, created_at, kind, tags, content set
// This function will compute the ID and signature
pub fn sign_event(event: &mut Event, secret_key_hex: &str) -> Result<(), String> {
    // Parse the secret key
    let secret_bytes = hex_to_bytes(secret_key_hex)?;
    
    if secret_bytes.len() != 32 {
        return Err(format!("Invalid secret key length: expected 32 bytes, got {}", secret_bytes.len()));
    }
    
    let secret_key = match SecretKey::from_slice(&secret_bytes) {
        Ok(sk) => sk,
        Err(e) => return Err(format!("Invalid secret key: {}", e)),
    };
    
    // Create keypair
    let secp = Secp256k1::new();
    let keypair = Keypair::from_secret_key(&secp, &secret_key);
    
    // Verify the public key matches
    let (xonly_pubkey, _parity) = XOnlyPublicKey::from_keypair(&keypair);
    let derived_pubkey = bytes_to_hex(&xonly_pubkey.serialize());
    
    if derived_pubkey.to_lowercase() != event.pubkey.to_lowercase() {
        return Err(format!(
            "Public key mismatch: event has {}, but secret key produces {}",
            event.pubkey, derived_pubkey
        ));
    }
    
    // Compute the event ID
    let event_id = compute_event_id(event)?;
    event.id = event_id.clone();
    
    // Get the ID as bytes for signing
    let id_bytes = hex_to_bytes(&event_id)?;
    
    // Create the message to sign
    let message = match secp256k1::Message::from_digest_slice(&id_bytes) {
        Ok(msg) => msg,
        Err(e) => return Err(format!("Failed to create message: {}", e)),
    };
    
    // Sign with Schnorr (no aux random data - deterministic)
    let signature = secp.sign_schnorr_no_aux_rand(&message, &keypair);
    
    // Store the signature
    event.sig = bytes_to_hex(signature.as_ref());
    
    return Ok(());
}

// Create and sign a new text note (kind 1)
pub fn create_signed_note(
    content: &str,
    secret_key_hex: &str,
    tags: Vec<Vec<String>>,
) -> Result<Event, String> {
    // Get public key from secret
    let pubkey = get_public_key_from_secret(secret_key_hex)?;
    
    // Get current timestamp
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    
    // Create the event structure
    let mut event = Event {
        id: String::new(),  // Will be computed
        pubkey: pubkey,
        created_at: created_at,
        kind: 1,  // Text note
        tags: tags,
        content: content.to_string(),
        sig: String::new(),  // Will be computed
    };
    
    // Sign the event
    sign_event(&mut event, secret_key_hex)?;
    
    return Ok(event);
}

/// Create and sign a kind 7 (reaction) event. NIP-25: tags ["e", event_id], ["p", author_pubkey]; content = emoji (e.g. "❤️" or "+").
pub fn create_signed_reaction(
    event_id: &str,
    author_pubkey: &str,
    content: &str,
    secret_key_hex: &str,
) -> Result<Event, String> {
    let pubkey = get_public_key_from_secret(secret_key_hex)?;
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let tags = vec![
        vec![String::from("e"), event_id.to_string()],
        vec![String::from("p"), author_pubkey.to_string()],
    ];
    let mut event = Event {
        id: String::new(),
        pubkey,
        created_at,
        kind: 7,
        tags,
        content: content.to_string(),
        sig: String::new(),
    };
    sign_event(&mut event, secret_key_hex)?;
    Ok(event)
}

/// Create and sign a kind 6 (repost) event. NIP-18: tags ["e", event_id], ["p", author_pubkey]; content empty or stringified original event.
pub fn create_signed_repost(
    event_id: &str,
    author_pubkey: &str,
    content: &str,
    secret_key_hex: &str,
) -> Result<Event, String> {
    let pubkey = get_public_key_from_secret(secret_key_hex)?;
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let tags = vec![
        vec![String::from("e"), event_id.to_string()],
        vec![String::from("p"), author_pubkey.to_string()],
    ];
    let mut event = Event {
        id: String::new(),
        pubkey,
        created_at,
        kind: 6,
        tags,
        content: content.to_string(),
        sig: String::new(),
    };
    sign_event(&mut event, secret_key_hex)?;
    Ok(event)
}

/// Create and sign a kind 9734 (zap request) event. NIP-57.
/// relay_urls: relays for the recipient to publish zap receipt; target_pubkey: recipient; event_id: optional note being zapped; amount_msats: millisatoshis; content: optional message.
pub fn create_signed_zap_request(
    relay_urls: &[String],
    target_pubkey: &str,
    event_id: Option<&str>,
    amount_msats: u64,
    content: &str,
    secret_key_hex: &str,
) -> Result<Event, String> {
    let pubkey = get_public_key_from_secret(secret_key_hex)?;
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut relay_tag = vec![String::from("relays")];
    relay_tag.extend(relay_urls.iter().cloned());
    let mut tags: Vec<Vec<String>> = vec![
        vec![String::from("p"), target_pubkey.to_string()],
        relay_tag,
        vec![String::from("amount"), amount_msats.to_string()],
    ];
    if let Some(eid) = event_id {
        if !eid.is_empty() {
            tags.insert(1, vec![String::from("e"), eid.to_string()]);
        }
    }
    let mut event = Event {
        id: String::new(),
        pubkey,
        created_at,
        kind: KIND_ZAP_REQUEST,
        tags,
        content: content.to_string(),
        sig: String::new(),
    };
    sign_event(&mut event, secret_key_hex)?;
    Ok(event)
}

/// Create and sign a kind 3 (contact list) event. Tags: ["p", pubkey] for each followed user; content empty.
pub fn create_signed_contact_list(pubkeys: &[String], secret_key_hex: &str) -> Result<Event, String> {
    let pubkey = get_public_key_from_secret(secret_key_hex)?;
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let tags: Vec<Vec<String>> = pubkeys
        .iter()
        .map(|p| vec![String::from("p"), p.clone()])
        .collect();
    let mut event = Event {
        id: String::new(),
        pubkey,
        created_at,
        kind: 3,
        tags,
        content: String::new(),
        sig: String::new(),
    };
    sign_event(&mut event, secret_key_hex)?;
    Ok(event)
}

/// Create and sign a kind 0 (metadata) event.
pub fn create_signed_metadata_event(content: &str, secret_key_hex: &str) -> Result<Event, String> {
    let pubkey = get_public_key_from_secret(secret_key_hex)?;
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut event = Event {
        id: String::new(),
        pubkey: pubkey,
        created_at: created_at,
        kind: 0,
        tags: Vec::new(),
        content: content.to_string(),
        sig: String::new(),
    };
    sign_event(&mut event, secret_key_hex)?;
    Ok(event)
}

/// Create and sign a kind 4 (NIP-04) encrypted DM. Encrypts content for recipient, tags ["p", recipient_pubkey].
pub fn create_signed_dm(
    recipient_pubkey_hex: &str,
    plaintext: &str,
    secret_key_hex: &str,
) -> Result<Event, String> {
    let encrypted = nip04_encrypt(plaintext, secret_key_hex, recipient_pubkey_hex)?;
    let pubkey = get_public_key_from_secret(secret_key_hex)?;
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let tags = vec![vec![String::from("p"), recipient_pubkey_hex.to_string()]];
    let mut event = Event {
        id: String::new(),
        pubkey,
        created_at,
        kind: KIND_DM,
        tags,
        content: encrypted,
        sig: String::new(),
    };
    sign_event(&mut event, secret_key_hex)?;
    Ok(event)
}

// ============================================================
// NIP-04 Encrypted Direct Messages
// ============================================================

/// Derive the 32-byte shared secret (X coordinate of ECDH point) for NIP-04.
/// our_secret_hex: our private key (hex); their_public_hex: other party's public key (32-byte hex).
fn nip04_shared_secret(our_secret_hex: &str, their_public_hex: &str) -> Result<[u8; 32], String> {
    let our_secret_bytes = hex_to_bytes(our_secret_hex)?;
    if our_secret_bytes.len() != 32 {
        return Err(String::from("Invalid secret key length"));
    }
    let their_pubkey_bytes = hex_to_bytes(their_public_hex)?;
    if their_pubkey_bytes.len() != 32 {
        return Err(String::from("Invalid public key length"));
    }

    let secret_key = SecretKey::from_slice(&our_secret_bytes)
        .map_err(|e| format!("Invalid secret key: {}", e))?;
    let xonly = XOnlyPublicKey::from_slice(&their_pubkey_bytes)
        .map_err(|e| format!("Invalid public key: {}", e))?;

    // Nostr uses x-only pubkeys; secp256k1 ECDH needs full PublicKey. Use even parity (standard).
    let public_key = PublicKey::from_x_only_public_key(xonly, Parity::Even);

    let point = shared_secret_point(&public_key, &secret_key);
    let mut key = [0u8; 32];
    key.copy_from_slice(&point[0..32]);
    Ok(key)
}

/// NIP-04 encrypt: AES-256-CBC with random IV. Returns "base64(ciphertext)?iv=base64(iv)".
pub fn nip04_encrypt(plaintext: &str, our_secret_hex: &str, their_public_hex: &str) -> Result<String, String> {
    let key = nip04_shared_secret(our_secret_hex, their_public_hex)?;
    let iv: [u8; 16] = rand::random();

    let mut buf = vec![0u8; plaintext.len() + 16];
    let len = plaintext.len();
    buf[..len].copy_from_slice(plaintext.as_bytes());

    let ciphertext = Aes256CbcEnc::new((&key).into(), (&iv).into())
        .encrypt_padded_mut::<Pkcs7>(&mut buf, len)
        .map_err(|_| String::from("Encryption failed"))?;

    let ct_b64 = BASE64.encode(ciphertext);
    let iv_b64 = BASE64.encode(iv);
    Ok(format!("{}?iv={}", ct_b64, iv_b64))
}

/// NIP-04 decrypt. content is "base64(ciphertext)?iv=base64(iv)".
pub fn nip04_decrypt(content: &str, our_secret_hex: &str, their_public_hex: &str) -> Result<String, String> {
    let key = nip04_shared_secret(our_secret_hex, their_public_hex)?;

    let parts: Vec<&str> = content.splitn(2, "?iv=").collect();
    if parts.len() != 2 {
        return Err(String::from("Invalid NIP-04 content format"));
    }
    let ct_b64 = parts[0].trim();
    let iv_b64 = parts[1].trim();

    let ciphertext = BASE64.decode(ct_b64).map_err(|e| format!("Invalid base64 ciphertext: {}", e))?;
    let iv: [u8; 16] = BASE64
        .decode(iv_b64)
        .map_err(|e| format!("Invalid base64 IV: {}", e))?
        .try_into()
        .map_err(|_| String::from("IV must be 16 bytes"))?;

    let mut buf = ciphertext.clone();
    let decrypted = Aes256CbcDec::new((&key).into(), (&iv).into())
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .map_err(|_| String::from("Decryption failed (wrong key or corrupted data)"))?;

    String::from_utf8(decrypted.to_vec()).map_err(|e| format!("Invalid UTF-8: {}", e))
}

// ============================================================
// NIP-44 Versioned Encryption (v2)
// ============================================================

type HmacSha256 = Hmac<Sha256>;

/// Derive the NIP-44 conversation key from ECDH shared secret + HKDF-extract.
/// Symmetric: conv_key(a, B) == conv_key(b, A).
pub fn nip44_conversation_key(our_secret_hex: &str, their_public_hex: &str) -> Result<[u8; 32], String> {
    let our_secret_bytes = hex_to_bytes(our_secret_hex)?;
    if our_secret_bytes.len() != 32 {
        return Err(String::from("Invalid secret key length"));
    }
    let their_pubkey_bytes = hex_to_bytes(their_public_hex)?;
    if their_pubkey_bytes.len() != 32 {
        return Err(String::from("Invalid public key length"));
    }

    let secret_key = SecretKey::from_slice(&our_secret_bytes)
        .map_err(|e| format!("Invalid secret key: {}", e))?;
    let xonly = XOnlyPublicKey::from_slice(&their_pubkey_bytes)
        .map_err(|e| format!("Invalid public key: {}", e))?;
    let public_key = PublicKey::from_x_only_public_key(xonly, Parity::Even);

    let point = shared_secret_point(&public_key, &secret_key);
    let shared_x = &point[0..32];

    let hk = Hkdf::<Sha256>::new(Some(b"nip44-v2"), shared_x);
    let mut conversation_key = [0u8; 32];
    hk.expand(&[], &mut conversation_key)
        .map_err(|_| String::from("HKDF expand failed for conversation key"))?;
    Ok(conversation_key)
}

/// Derive per-message keys (chacha_key, chacha_nonce, hmac_key) from conversation_key and nonce.
fn nip44_message_keys(conversation_key: &[u8; 32], nonce: &[u8; 32]) -> Result<([u8; 32], [u8; 12], [u8; 32]), String> {
    let hk = Hkdf::<Sha256>::new(Some(conversation_key), &[]);
    let mut keys = [0u8; 76];
    hk.expand(nonce, &mut keys)
        .map_err(|_| String::from("HKDF expand failed for message keys"))?;
    let mut chacha_key = [0u8; 32];
    chacha_key.copy_from_slice(&keys[0..32]);
    let mut chacha_nonce = [0u8; 12];
    chacha_nonce.copy_from_slice(&keys[32..44]);
    let mut hmac_key = [0u8; 32];
    hmac_key.copy_from_slice(&keys[44..76]);
    Ok((chacha_key, chacha_nonce, hmac_key))
}

/// NIP-44 padding length calculation (power-of-two based, min 32).
pub fn nip44_calc_padded_len(unpadded_len: usize) -> Result<usize, String> {
    if unpadded_len < 1 {
        return Err(String::from("Plaintext must be at least 1 byte"));
    }
    if unpadded_len > 65535 {
        return Err(String::from("Plaintext must be at most 65535 bytes"));
    }
    if unpadded_len <= 32 {
        return Ok(32);
    }
    let next_power = 1usize << (usize::BITS - (unpadded_len - 1).leading_zeros());
    let chunk = if next_power <= 256 { 32 } else { next_power / 8 };
    Ok(chunk * (((unpadded_len - 1) / chunk) + 1))
}

/// Pad plaintext per NIP-44: [u16be length][plaintext][zero padding].
fn nip44_pad(plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let unpadded_len = plaintext.len();
    let padded_len = nip44_calc_padded_len(unpadded_len)?;
    let mut padded = Vec::with_capacity(2 + padded_len);
    padded.push((unpadded_len >> 8) as u8);
    padded.push((unpadded_len & 0xff) as u8);
    padded.extend_from_slice(plaintext);
    padded.resize(2 + padded_len, 0);
    Ok(padded)
}

/// Unpad per NIP-44: read u16be length prefix, validate, return plaintext.
fn nip44_unpad(padded: &[u8]) -> Result<String, String> {
    if padded.len() < 2 {
        return Err(String::from("Padded data too short"));
    }
    let unpadded_len = ((padded[0] as usize) << 8) | (padded[1] as usize);
    if unpadded_len == 0 {
        return Err(String::from("Invalid padding: zero length"));
    }
    if 2 + unpadded_len > padded.len() {
        return Err(String::from("Invalid padding: length exceeds data"));
    }
    let expected_padded_len = nip44_calc_padded_len(unpadded_len)?;
    if padded.len() != 2 + expected_padded_len {
        return Err(String::from("Invalid padding: unexpected padded size"));
    }
    let plaintext = &padded[2..2 + unpadded_len];
    String::from_utf8(plaintext.to_vec()).map_err(|e| format!("Invalid UTF-8: {}", e))
}

/// HMAC-SHA256 over AAD (nonce) || message, per NIP-44.
fn nip44_hmac_aad(hmac_key: &[u8; 32], message: &[u8], aad: &[u8; 32]) -> Result<[u8; 32], String> {
    let mut mac = HmacSha256::new_from_slice(hmac_key)
        .map_err(|_| String::from("HMAC key error"))?;
    mac.update(aad);
    mac.update(message);
    let result = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    Ok(out)
}

/// NIP-44 v2 encrypt. Returns base64(0x02 || nonce || ciphertext || mac).
pub fn nip44_encrypt(plaintext: &str, conversation_key: &[u8; 32]) -> Result<String, String> {
    let plaintext_bytes = plaintext.as_bytes();
    if plaintext_bytes.is_empty() || plaintext_bytes.len() > 65535 {
        return Err(String::from("Plaintext length out of range (1..65535)"));
    }

    let nonce: [u8; 32] = rand::random();
    let (chacha_key, chacha_nonce, hmac_key) = nip44_message_keys(conversation_key, &nonce)?;

    let padded = nip44_pad(plaintext_bytes)?;

    let mut ciphertext = padded;
    let mut cipher = chacha20::ChaCha20::new((&chacha_key).into(), (&chacha_nonce).into());
    cipher.apply_keystream(&mut ciphertext);

    let mac = nip44_hmac_aad(&hmac_key, &ciphertext, &nonce)?;

    let mut payload = Vec::with_capacity(1 + 32 + ciphertext.len() + 32);
    payload.push(0x02); // version
    payload.extend_from_slice(&nonce);
    payload.extend_from_slice(&ciphertext);
    payload.extend_from_slice(&mac);

    Ok(BASE64.encode(&payload))
}

/// NIP-44 v2 decrypt. Payload is base64(0x02 || nonce || ciphertext || mac).
pub fn nip44_decrypt(payload: &str, conversation_key: &[u8; 32]) -> Result<String, String> {
    if payload.is_empty() {
        return Err(String::from("Empty payload"));
    }
    if payload.starts_with('#') {
        return Err(String::from("Unsupported encryption version"));
    }

    let plen = payload.len();
    if plen < 132 || plen > 87472 {
        return Err(String::from("Invalid payload size"));
    }

    let data = BASE64.decode(payload).map_err(|e| format!("Invalid base64: {}", e))?;
    let dlen = data.len();
    if dlen < 99 || dlen > 65603 {
        return Err(String::from("Invalid decoded data size"));
    }
    if data[0] != 0x02 {
        return Err(format!("Unknown encryption version: {}", data[0]));
    }

    let nonce: [u8; 32] = data[1..33].try_into()
        .map_err(|_| String::from("Invalid nonce"))?;
    let ciphertext = &data[33..dlen - 32];
    let mac: [u8; 32] = data[dlen - 32..dlen].try_into()
        .map_err(|_| String::from("Invalid MAC"))?;

    let (chacha_key, chacha_nonce, hmac_key) = nip44_message_keys(conversation_key, &nonce)?;

    let expected_mac = nip44_hmac_aad(&hmac_key, ciphertext, &nonce)?;
    if !constant_time_eq(&mac, &expected_mac) {
        return Err(String::from("Invalid MAC"));
    }

    let mut padded = ciphertext.to_vec();
    let mut cipher = chacha20::ChaCha20::new((&chacha_key).into(), (&chacha_nonce).into());
    cipher.apply_keystream(&mut padded);

    nip44_unpad(&padded)
}

/// Constant-time comparison to prevent timing attacks on MAC verification.
fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let mut diff = 0u8;
    for i in 0..32 {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

// ============================================================
// NIP-59 Gift Wrap (Rumor / Seal / Gift Wrap)
// ============================================================

/// Random timestamp in the past (up to 2 days) to mask real event timing.
fn random_past_timestamp() -> u64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let jitter: u64 = rand::random::<u64>() % 172800; // 0..2 days
    now.saturating_sub(jitter)
}

/// Create a kind 14 rumor (unsigned event). The id is computed but sig is empty.
pub fn create_rumor(
    content: &str,
    tags: Vec<Vec<String>>,
    sender_pubkey_hex: &str,
) -> Result<Event, String> {
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let mut event = Event {
        id: String::new(),
        pubkey: sender_pubkey_hex.to_string(),
        created_at,
        kind: KIND_CHAT_MESSAGE,
        tags,
        content: content.to_string(),
        sig: String::new(),
    };
    event.id = compute_event_id(&event)?;
    Ok(event)
}

/// Create a kind 13 seal: encrypts the rumor JSON with NIP-44 for the recipient.
/// Signed by the sender, with a randomized timestamp and no tags.
pub fn create_seal(
    rumor: &Event,
    sender_secret_hex: &str,
    recipient_pubkey_hex: &str,
) -> Result<Event, String> {
    let sender_pubkey = get_public_key_from_secret(sender_secret_hex)?;
    let conv_key = nip44_conversation_key(sender_secret_hex, recipient_pubkey_hex)?;
    let rumor_json = event_to_json_compact(rumor);
    let encrypted = nip44_encrypt(&rumor_json, &conv_key)?;

    let mut seal = Event {
        id: String::new(),
        pubkey: sender_pubkey,
        created_at: random_past_timestamp(),
        kind: KIND_SEAL,
        tags: Vec::new(),
        content: encrypted,
        sig: String::new(),
    };
    sign_event(&mut seal, sender_secret_hex)?;
    Ok(seal)
}

/// Create a kind 1059 gift wrap: encrypts the seal JSON with NIP-44 using an ephemeral key.
/// Addressed to the recipient via a ["p", recipient] tag.
pub fn create_gift_wrap(
    seal: &Event,
    recipient_pubkey_hex: &str,
) -> Result<Event, String> {
    let (eph_secret, eph_pubkey) = generate_keypair()?;
    let conv_key = nip44_conversation_key(&eph_secret, recipient_pubkey_hex)?;
    let seal_json = event_to_json_compact(seal);
    let encrypted = nip44_encrypt(&seal_json, &conv_key)?;

    let mut wrap = Event {
        id: String::new(),
        pubkey: eph_pubkey,
        created_at: random_past_timestamp(),
        kind: KIND_GIFT_WRAP,
        tags: vec![vec![String::from("p"), recipient_pubkey_hex.to_string()]],
        content: encrypted,
        sig: String::new(),
    };
    sign_event(&mut wrap, &eph_secret)?;
    Ok(wrap)
}

/// Unwrap a kind 1059 gift wrap: decrypt outer (gift wrap -> seal), then inner (seal -> rumor).
/// Returns (seal, rumor). Verifies seal signature and anti-impersonation (rumor.pubkey == seal.pubkey).
pub fn unwrap_gift_wrap(gift_wrap: &Event, our_secret_hex: &str) -> Result<(Event, Event), String> {
    if gift_wrap.kind != KIND_GIFT_WRAP {
        return Err(format!("Expected kind 1059, got kind {}", gift_wrap.kind));
    }

    // Outer layer: decrypt with conv_key(our_secret, gift_wrap.pubkey)
    let outer_conv = nip44_conversation_key(our_secret_hex, &gift_wrap.pubkey)?;
    let seal_json = nip44_decrypt(&gift_wrap.content, &outer_conv)?;
    let seal = parse_event(&seal_json)?;

    if seal.kind != KIND_SEAL {
        return Err(format!("Expected seal kind 13, got kind {}", seal.kind));
    }

    // Verify seal signature
    let seal_valid = verify_event_signature(&seal)?;
    if !seal_valid {
        return Err(String::from("Seal signature verification failed"));
    }

    // Inner layer: decrypt with conv_key(our_secret, seal.pubkey)
    let inner_conv = nip44_conversation_key(our_secret_hex, &seal.pubkey)?;
    let rumor_json = nip44_decrypt(&seal.content, &inner_conv)?;
    let rumor = parse_event(&rumor_json)?;

    // Anti-impersonation: rumor author must match seal signer
    if rumor.pubkey.to_lowercase() != seal.pubkey.to_lowercase() {
        return Err(String::from("Rumor pubkey does not match seal pubkey (impersonation detected)"));
    }

    Ok((seal, rumor))
}

/// Build the full NIP-17 gift wrap chain for a private message.
/// Returns two gift wraps: one for the recipient and one for ourselves (self-copy).
pub fn create_nip17_dm(
    plaintext: &str,
    sender_secret_hex: &str,
    recipient_pubkey_hex: &str,
) -> Result<(Event, Event), String> {
    let sender_pubkey = get_public_key_from_secret(sender_secret_hex)?;
    let tags = vec![vec![String::from("p"), recipient_pubkey_hex.to_string()]];
    let rumor = create_rumor(plaintext, tags, &sender_pubkey)?;

    let seal = create_seal(&rumor, sender_secret_hex, recipient_pubkey_hex)?;
    let wrap_for_recipient = create_gift_wrap(&seal, recipient_pubkey_hex)?;

    let seal_self = create_seal(&rumor, sender_secret_hex, &sender_pubkey)?;
    let wrap_for_self = create_gift_wrap(&seal_self, &sender_pubkey)?;

    Ok((wrap_for_recipient, wrap_for_self))
}

/// Create and sign a kind 10050 (DM relay list) event.
pub fn create_signed_dm_relay_list(
    relay_urls: &[String],
    secret_key_hex: &str,
) -> Result<Event, String> {
    let pubkey = get_public_key_from_secret(secret_key_hex)?;
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let tags: Vec<Vec<String>> = relay_urls
        .iter()
        .map(|url| vec![String::from("relay"), url.clone()])
        .collect();
    let mut event = Event {
        id: String::new(),
        pubkey,
        created_at,
        kind: crate::nostr::KIND_DM_RELAY_LIST,
        tags,
        content: String::new(),
        sig: String::new(),
    };
    sign_event(&mut event, secret_key_hex)?;
    Ok(event)
}

// ============================================================
// NIP-98 / Blossom Auth Events
// ============================================================

/// Create and sign a NIP-98 HTTP auth event (kind 27235).
/// Tags: ["u", url], ["method", method], optionally ["payload", sha256_hex].
pub fn create_nip98_auth_event(
    url: &str,
    method: &str,
    payload_hash: Option<&str>,
    secret_key_hex: &str,
) -> Result<Event, String> {
    let pubkey = get_public_key_from_secret(secret_key_hex)?;
    let created_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mut tags: Vec<Vec<String>> = vec![
        vec![String::from("u"), url.to_string()],
        vec![String::from("method"), method.to_uppercase()],
    ];
    if let Some(hash) = payload_hash {
        tags.push(vec![String::from("payload"), hash.to_string()]);
    }
    let mut event = Event {
        id: String::new(),
        pubkey,
        created_at,
        kind: KIND_HTTP_AUTH,
        tags,
        content: String::new(),
        sig: String::new(),
    };
    sign_event(&mut event, secret_key_hex)?;
    Ok(event)
}

/// Create and sign a Blossom auth event (kind 24242).
/// action: "upload" or "delete"; file_hash: SHA-256 hex of the file; expiration: seconds from now.
pub fn create_blossom_auth_event(
    action: &str,
    file_hash: &str,
    secret_key_hex: &str,
) -> Result<Event, String> {
    let pubkey = get_public_key_from_secret(secret_key_hex)?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let expiration = now + 600; // 10-minute validity
    let tags: Vec<Vec<String>> = vec![
        vec![String::from("t"), action.to_string()],
        vec![String::from("x"), file_hash.to_string()],
        vec![String::from("expiration"), expiration.to_string()],
    ];
    let mut event = Event {
        id: String::new(),
        pubkey,
        created_at: now,
        kind: KIND_BLOSSOM_AUTH,
        tags,
        content: format!("{} {}", action, file_hash),
        sig: String::new(),
    };
    sign_event(&mut event, secret_key_hex)?;
    Ok(event)
}

/// Compute SHA-256 hash of raw bytes, returning hex string.
pub fn sha256_hex(data: &[u8]) -> String {
    bytes_to_hex(&sha256_hash(data))
}

// ============================================================
// Helper Functions
// ============================================================

// Compute SHA256 hash
fn sha256_hash(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let result = hasher.finalize();
    
    let mut hash: [u8; 32] = [0; 32];
    hash.copy_from_slice(&result);
    return hash;
}

// Convert hex string to bytes
fn hex_to_bytes(hex: &str) -> Result<Vec<u8>, String> {
    let mut bytes: Vec<u8> = Vec::new();
    let chars: Vec<char> = hex.chars().collect();
    
    if chars.len() % 2 != 0 {
        return Err(String::from("Hex string must have even length"));
    }
    
    let mut index = 0;
    while index < chars.len() {
        let high = match hex_char_to_value(chars[index]) {
            Some(v) => v,
            None => return Err(format!("Invalid hex character: {}", chars[index])),
        };
        
        let low = match hex_char_to_value(chars[index + 1]) {
            Some(v) => v,
            None => return Err(format!("Invalid hex character: {}", chars[index + 1])),
        };
        
        let byte = (high << 4) | low;
        bytes.push(byte);
        index = index + 2;
    }
    
    return Ok(bytes);
}

// Convert single hex character to value
fn hex_char_to_value(c: char) -> Option<u8> {
    match c {
        '0' => Some(0),
        '1' => Some(1),
        '2' => Some(2),
        '3' => Some(3),
        '4' => Some(4),
        '5' => Some(5),
        '6' => Some(6),
        '7' => Some(7),
        '8' => Some(8),
        '9' => Some(9),
        'a' | 'A' => Some(10),
        'b' | 'B' => Some(11),
        'c' | 'C' => Some(12),
        'd' | 'D' => Some(13),
        'e' | 'E' => Some(14),
        'f' | 'F' => Some(15),
        _ => None,
    }
}

// Convert bytes to hex string
fn bytes_to_hex(bytes: &[u8]) -> String {
    let hex_chars = ['0', '1', '2', '3', '4', '5', '6', '7',
                     '8', '9', 'a', 'b', 'c', 'd', 'e', 'f'];
    
    let mut result = String::new();
    
    for byte in bytes {
        let high = (byte >> 4) & 0x0F;
        let low = byte & 0x0F;
        result.push(hex_chars[high as usize]);
        result.push(hex_chars[low as usize]);
    }
    
    return result;
}

// Escape special characters in a string for JSON
fn escape_json_string(input: &str) -> String {
    let mut output = String::new();
    
    for c in input.chars() {
        match c {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            // Handle control characters (0x00 to 0x1F)
            c if (c as u32) < 0x20 => {
                output.push_str(&format!("\\u{:04x}", c as u32));
            }
            _ => output.push(c),
        }
    }
    
    return output;
}

// ============================================================
// Tests (can be run with cargo test)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_sha256() {
        // Test vector from Bitcoin
        let input = b"hello";
        let hash = sha256_hash(input);
        let hex = bytes_to_hex(&hash);
        assert_eq!(hex, "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824");
    }
    
    #[test]
    fn test_hex_conversion() {
        let original = "deadbeef";
        let bytes = hex_to_bytes(original).unwrap();
        let back = bytes_to_hex(&bytes);
        assert_eq!(original, back);
    }

    // NIP-44 test vectors from https://github.com/paulmillr/nip44
    #[test]
    fn test_nip44_conversation_key() {
        let result = nip44_conversation_key(
            "0000000000000000000000000000000000000000000000000000000000000001",
            "0000000000000000000000000000000000000000000000000000000000000000",
        );
        assert!(result.is_err(), "Zero point should be rejected");

        let (sec_a, pub_a) = generate_keypair().unwrap();
        let (sec_b, pub_b) = generate_keypair().unwrap();
        let ck_ab = nip44_conversation_key(&sec_a, &pub_b).unwrap();
        let ck_ba = nip44_conversation_key(&sec_b, &pub_a).unwrap();
        assert_eq!(ck_ab, ck_ba, "Conversation key must be symmetric");
    }

    #[test]
    fn test_nip44_padding() {
        assert_eq!(nip44_calc_padded_len(1).unwrap(), 32);
        assert_eq!(nip44_calc_padded_len(16).unwrap(), 32);
        assert_eq!(nip44_calc_padded_len(32).unwrap(), 32);
        assert_eq!(nip44_calc_padded_len(33).unwrap(), 64);
        assert_eq!(nip44_calc_padded_len(64).unwrap(), 64);
        assert_eq!(nip44_calc_padded_len(65).unwrap(), 96);
        assert_eq!(nip44_calc_padded_len(100).unwrap(), 128);
        assert_eq!(nip44_calc_padded_len(256).unwrap(), 256);
        assert_eq!(nip44_calc_padded_len(257).unwrap(), 320);
        assert_eq!(nip44_calc_padded_len(320).unwrap(), 320);
        assert_eq!(nip44_calc_padded_len(65535).unwrap(), 65536);
        assert!(nip44_calc_padded_len(0).is_err());
        assert!(nip44_calc_padded_len(65536).is_err());
    }

    #[test]
    fn test_nip44_pad_unpad_roundtrip() {
        for text in &["a", "hello world", &"x".repeat(32), &"y".repeat(33), &"z".repeat(1000)] {
            let padded = nip44_pad(text.as_bytes()).unwrap();
            let unpadded = nip44_unpad(&padded).unwrap();
            assert_eq!(&unpadded, text);
        }
    }

    #[test]
    fn test_nip44_encrypt_decrypt_roundtrip() {
        let (sec_a, pub_a) = generate_keypair().unwrap();
        let (sec_b, pub_b) = generate_keypair().unwrap();
        let ck = nip44_conversation_key(&sec_a, &pub_b).unwrap();
        let ck2 = nip44_conversation_key(&sec_b, &pub_a).unwrap();
        assert_eq!(ck, ck2);

        let long_100 = "x".repeat(100);
        let long_max = "y".repeat(65535);
        let messages = vec!["hello", "a", &long_100, &long_max];
        for msg in messages {
            let encrypted = nip44_encrypt(msg, &ck).unwrap();
            let decrypted = nip44_decrypt(&encrypted, &ck).unwrap();
            assert_eq!(decrypted, msg);
        }
    }

    #[test]
    fn test_nip44_wrong_key_fails() {
        let (sec_a, _pub_a) = generate_keypair().unwrap();
        let (_sec_b, pub_b) = generate_keypair().unwrap();
        let (_sec_c, pub_c) = generate_keypair().unwrap();
        let ck_correct = nip44_conversation_key(&sec_a, &pub_b).unwrap();
        let ck_wrong = nip44_conversation_key(&sec_a, &pub_c).unwrap();

        let encrypted = nip44_encrypt("secret message", &ck_correct).unwrap();
        let result = nip44_decrypt(&encrypted, &ck_wrong);
        assert!(result.is_err(), "Decryption with wrong key should fail MAC check");
    }

    #[test]
    fn test_nip44_version_byte() {
        let (sec_a, _pub_a) = generate_keypair().unwrap();
        let (_sec_b, pub_b) = generate_keypair().unwrap();
        let ck = nip44_conversation_key(&sec_a, &pub_b).unwrap();

        let encrypted = nip44_encrypt("test", &ck).unwrap();
        let decoded = BASE64.decode(&encrypted).unwrap();
        assert_eq!(decoded[0], 0x02, "First byte must be version 0x02");
    }

    // NIP-59 tests
    #[test]
    fn test_nip59_gift_wrap_roundtrip() {
        let (sec_alice, pub_alice) = generate_keypair().unwrap();
        let (sec_bob, pub_bob) = generate_keypair().unwrap();

        let (wrap_for_bob, wrap_for_alice) =
            create_nip17_dm("Hello Bob!", &sec_alice, &pub_bob).unwrap();

        assert_eq!(wrap_for_bob.kind, 1059);
        assert_eq!(wrap_for_alice.kind, 1059);

        // Bob unwraps his copy
        let (_seal_b, rumor_b) = unwrap_gift_wrap(&wrap_for_bob, &sec_bob).unwrap();
        assert_eq!(rumor_b.content, "Hello Bob!");
        assert_eq!(rumor_b.pubkey.to_lowercase(), pub_alice.to_lowercase());
        assert_eq!(rumor_b.kind, 14);

        // Alice unwraps her self-copy
        let (_seal_a, rumor_a) = unwrap_gift_wrap(&wrap_for_alice, &sec_alice).unwrap();
        assert_eq!(rumor_a.content, "Hello Bob!");
        assert_eq!(rumor_a.pubkey.to_lowercase(), pub_alice.to_lowercase());

        // Both get the same rumor ID
        assert_eq!(rumor_a.id, rumor_b.id);
    }

    #[test]
    fn test_nip59_wrong_recipient_fails() {
        let (sec_alice, _pub_alice) = generate_keypair().unwrap();
        let (sec_bob, pub_bob) = generate_keypair().unwrap();
        let (sec_charlie, _pub_charlie) = generate_keypair().unwrap();

        let (wrap_for_bob, _) =
            create_nip17_dm("Secret", &sec_alice, &pub_bob).unwrap();

        // Charlie cannot unwrap Bob's gift wrap
        let result = unwrap_gift_wrap(&wrap_for_bob, &sec_charlie);
        assert!(result.is_err());

        // Bob can
        let (_seal, rumor) = unwrap_gift_wrap(&wrap_for_bob, &sec_bob).unwrap();
        assert_eq!(rumor.content, "Secret");
    }

    #[test]
    fn test_nip59_gift_wrap_unique_ids() {
        let (sec_alice, _pub_alice) = generate_keypair().unwrap();
        let (_sec_bob, pub_bob) = generate_keypair().unwrap();

        let (wrap1, _) = create_nip17_dm("msg1", &sec_alice, &pub_bob).unwrap();
        let (wrap2, _) = create_nip17_dm("msg2", &sec_alice, &pub_bob).unwrap();

        assert_ne!(wrap1.id, wrap2.id);
        assert_ne!(wrap1.pubkey, wrap2.pubkey, "Each wrap uses a unique ephemeral key");
    }

    #[test]
    fn test_dm_relay_list_event() {
        let (sec, _pub) = generate_keypair().unwrap();
        let relays = vec![
            String::from("wss://relay.damus.io"),
            String::from("wss://relay.primal.net"),
        ];
        let event = create_signed_dm_relay_list(&relays, &sec).unwrap();
        assert_eq!(event.kind, 10050);
        assert_eq!(event.tags.len(), 2);
        assert_eq!(event.tags[0][0], "relay");
        assert_eq!(event.tags[0][1], "wss://relay.damus.io");
        assert_eq!(event.tags[1][0], "relay");
        assert_eq!(event.tags[1][1], "wss://relay.primal.net");
    }
}

