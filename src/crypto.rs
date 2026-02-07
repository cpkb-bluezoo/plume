// Plume - Cryptographic Operations
// Handles signature verification and signing for Nostr events
//
// Nostr uses secp256k1 Schnorr signatures (BIP-340)
// Event IDs are SHA256 hashes of the serialized event

use secp256k1::{schnorr, Keypair, Secp256k1, SecretKey, XOnlyPublicKey};
use sha2::{Digest, Sha256};

use crate::nostr::Event;

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
}

