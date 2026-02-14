/*
 * keys.rs
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

// Nostr uses two key formats:
// - Hex: 64 character hexadecimal string (32 bytes)
// - Bech32: Human-readable format with error detection
//   - npub1... for public keys
//   - nsec1... for private keys (secrets)
// See: https://github.com/nostr-protocol/nips/blob/master/19.md

use bech32::{Bech32, Hrp};

// Human-readable parts for Nostr keys
const HRP_PUBLIC_KEY: &str = "npub";
const HRP_SECRET_KEY: &str = "nsec";

// ============================================================
// Validation Functions
// ============================================================

// Check if a string is a valid 64-character hex string (32 bytes)
pub fn is_valid_hex_key(key: &str) -> bool {
    // Must be exactly 64 characters
    if key.len() != 64 {
        return false;
    }
    
    // All characters must be valid hexadecimal
    for character in key.chars() {
        let is_hex = (character >= '0' && character <= '9')
            || (character >= 'a' && character <= 'f')
            || (character >= 'A' && character <= 'F');
        
        if !is_hex {
            return false;
        }
    }
    
    return true;
}

// Check if a string looks like an npub (public key in bech32)
pub fn is_npub(key: &str) -> bool {
    return key.starts_with("npub1");
}

// Check if a string looks like an nsec (secret key in bech32)
pub fn is_nsec(key: &str) -> bool {
    return key.starts_with("nsec1");
}

/// Check if a string is a valid npub (decodes successfully).
#[allow(dead_code)]
pub fn is_valid_npub(key: &str) -> bool {
    if !is_npub(key) {
        return false;
    }
    
    // Try to decode it
    match npub_to_hex(key) {
        Ok(_) => return true,
        Err(_) => return false,
    }
}

/// Check if a string is a valid nsec (decodes successfully).
#[allow(dead_code)]
pub fn is_valid_nsec(key: &str) -> bool {
    if !is_nsec(key) {
        return false;
    }
    
    // Try to decode it
    match nsec_to_hex(key) {
        Ok(_) => return true,
        Err(_) => return false,
    }
}

// ============================================================
// Conversion: Hex to Bech32
// ============================================================

// Convert a hex public key to npub format
pub fn hex_to_npub(hex_key: &str) -> Result<String, String> {
    // Validate the hex key first
    if !is_valid_hex_key(hex_key) {
        return Err(String::from("Invalid hex key: must be 64 hex characters"));
    }
    
    // Convert hex string to bytes
    let bytes = match hex_to_bytes(hex_key) {
        Ok(b) => b,
        Err(e) => return Err(e),
    };
    
    // Create the human-readable part
    let hrp = match Hrp::parse(HRP_PUBLIC_KEY) {
        Ok(h) => h,
        Err(e) => return Err(format!("Failed to create HRP: {}", e)),
    };
    
    // Encode to bech32
    match bech32::encode::<Bech32>(hrp, &bytes) {
        Ok(encoded) => return Ok(encoded),
        Err(e) => return Err(format!("Bech32 encoding failed: {}", e)),
    }
}

// Convert a hex secret key to nsec format
pub fn hex_to_nsec(hex_key: &str) -> Result<String, String> {
    // Validate the hex key first
    if !is_valid_hex_key(hex_key) {
        return Err(String::from("Invalid hex key: must be 64 hex characters"));
    }
    
    // Convert hex string to bytes
    let bytes = match hex_to_bytes(hex_key) {
        Ok(b) => b,
        Err(e) => return Err(e),
    };
    
    // Create the human-readable part
    let hrp = match Hrp::parse(HRP_SECRET_KEY) {
        Ok(h) => h,
        Err(e) => return Err(format!("Failed to create HRP: {}", e)),
    };
    
    // Encode to bech32
    match bech32::encode::<Bech32>(hrp, &bytes) {
        Ok(encoded) => return Ok(encoded),
        Err(e) => return Err(format!("Bech32 encoding failed: {}", e)),
    }
}

// ============================================================
// Conversion: Bech32 to Hex
// ============================================================

// Convert an npub to hex public key
pub fn npub_to_hex(npub: &str) -> Result<String, String> {
    // Check prefix
    if !is_npub(npub) {
        return Err(String::from("Not an npub: must start with 'npub1'"));
    }
    
    // Decode the bech32 string
    let (hrp, bytes) = match bech32::decode(npub) {
        Ok(result) => result,
        Err(e) => return Err(format!("Invalid bech32: {}", e)),
    };
    
    // Verify the human-readable part
    if hrp.as_str() != HRP_PUBLIC_KEY {
        return Err(format!("Wrong prefix: expected '{}', got '{}'", HRP_PUBLIC_KEY, hrp));
    }
    
    // Verify length (should be 32 bytes)
    if bytes.len() != 32 {
        return Err(format!("Invalid key length: expected 32 bytes, got {}", bytes.len()));
    }
    
    // Convert bytes to hex string
    let hex = bytes_to_hex(&bytes);
    
    return Ok(hex);
}

// Convert an nsec to hex secret key
pub fn nsec_to_hex(nsec: &str) -> Result<String, String> {
    // Check prefix
    if !is_nsec(nsec) {
        return Err(String::from("Not an nsec: must start with 'nsec1'"));
    }
    
    // Decode the bech32 string
    let (hrp, bytes) = match bech32::decode(nsec) {
        Ok(result) => result,
        Err(e) => return Err(format!("Invalid bech32: {}", e)),
    };
    
    // Verify the human-readable part
    if hrp.as_str() != HRP_SECRET_KEY {
        return Err(format!("Wrong prefix: expected '{}', got '{}'", HRP_SECRET_KEY, hrp));
    }
    
    // Verify length (should be 32 bytes)
    if bytes.len() != 32 {
        return Err(format!("Invalid key length: expected 32 bytes, got {}", bytes.len()));
    }
    
    // Convert bytes to hex string
    let hex = bytes_to_hex(&bytes);
    
    return Ok(hex);
}

// ============================================================
// Smart Conversion (auto-detect format)
// ============================================================

// Convert any public key format to hex
// Accepts: npub1..., or 64-char hex
pub fn public_key_to_hex(key: &str) -> Result<String, String> {
    let trimmed = key.trim();
    
    if is_npub(trimmed) {
        return npub_to_hex(trimmed);
    } else if is_valid_hex_key(trimmed) {
        return Ok(trimmed.to_lowercase());
    } else {
        return Err(String::from("Invalid public key: must be npub1... or 64-char hex"));
    }
}

// Convert any secret key format to hex
// Accepts: nsec1..., or 64-char hex
pub fn secret_key_to_hex(key: &str) -> Result<String, String> {
    let trimmed = key.trim();
    
    if is_nsec(trimmed) {
        return nsec_to_hex(trimmed);
    } else if is_valid_hex_key(trimmed) {
        return Ok(trimmed.to_lowercase());
    } else {
        return Err(String::from("Invalid secret key: must be nsec1... or 64-char hex"));
    }
}

// ============================================================
// Helper Functions
// ============================================================

// Convert a hex string to bytes
fn hex_to_bytes(hex: &str) -> Result<Vec<u8>, String> {
    let mut bytes: Vec<u8> = Vec::new();
    
    // Process two characters at a time
    let chars: Vec<char> = hex.chars().collect();
    
    if chars.len() % 2 != 0 {
        return Err(String::from("Hex string must have even length"));
    }
    
    let mut index = 0;
    while index < chars.len() {
        let high_char = chars[index];
        let low_char = chars[index + 1];
        
        let high = match hex_char_to_value(high_char) {
            Some(v) => v,
            None => return Err(format!("Invalid hex character: {}", high_char)),
        };
        
        let low = match hex_char_to_value(low_char) {
            Some(v) => v,
            None => return Err(format!("Invalid hex character: {}", low_char)),
        };
        
        let byte = (high << 4) | low;
        bytes.push(byte);
        
        index = index + 2;
    }
    
    return Ok(bytes);
}

// Convert a single hex character to its value (0-15)
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

// ============================================================
// Display Helpers
// ============================================================

/// Shorten a key for display (first 8 + last 4 characters).
#[allow(dead_code)]
pub fn shorten_key(key: &str) -> String {
    if key.len() <= 16 {
        return key.to_string();
    }
    
    let start = &key[0..8];
    let end = &key[key.len()-4..];
    
    return format!("{}...{}", start, end);
}

/// Shorten an npub for display.
#[allow(dead_code)]
pub fn shorten_npub(npub: &str) -> String {
    if npub.len() <= 20 {
        return npub.to_string();
    }
    
    // npub1 + first 4 data chars + ... + last 4 chars
    let start = &npub[0..9];  // "npub1" + 4 chars
    let end = &npub[npub.len()-4..];
    
    return format!("{}...{}", start, end);
}

// ============================================================
// NIP-19 TLV Decoding (nevent, nprofile, note)
// ============================================================

// Human-readable parts for NIP-19 shareable identifiers
const HRP_NOTE: &str = "note";
const HRP_NEVENT: &str = "nevent";
const HRP_NPROFILE: &str = "nprofile";

// TLV type constants (NIP-19)
const TLV_SPECIAL: u8 = 0;  // event id (nevent) or pubkey (nprofile)
const TLV_RELAY: u8 = 1;    // relay URL (UTF-8)
const TLV_AUTHOR: u8 = 2;   // author pubkey (32 bytes, nevent only)

/// Decoded nevent: event ID + optional relay hints + optional author
pub struct DecodedNevent {
    pub event_id: String,       // hex
    pub relays: Vec<String>,
    pub author: Option<String>, // hex
}

/// Decoded nprofile: pubkey + optional relay hints
pub struct DecodedNprofile {
    pub pubkey: String,         // hex
    pub relays: Vec<String>,
}

/// Decode a note1... bech32 string to a hex event ID (simple 32-byte encoding, no TLV)
pub fn note_to_hex(note: &str) -> Result<String, String> {
    if !note.starts_with("note1") {
        return Err(String::from("Not a note: must start with 'note1'"));
    }
    let (hrp, bytes) = match bech32::decode(note) {
        Ok(result) => result,
        Err(e) => return Err(format!("Invalid bech32: {}", e)),
    };
    if hrp.as_str() != HRP_NOTE {
        return Err(format!("Wrong prefix: expected '{}', got '{}'", HRP_NOTE, hrp));
    }
    if bytes.len() != 32 {
        return Err(format!("Invalid event ID length: expected 32 bytes, got {}", bytes.len()));
    }
    Ok(bytes_to_hex(&bytes))
}

/// Decode an nevent1... bech32 string using the NIP-19 TLV format
pub fn decode_nevent(nevent: &str) -> Result<DecodedNevent, String> {
    if !nevent.starts_with("nevent1") {
        return Err(String::from("Not an nevent: must start with 'nevent1'"));
    }
    let (hrp, bytes) = match bech32::decode(nevent) {
        Ok(result) => result,
        Err(e) => return Err(format!("Invalid bech32: {}", e)),
    };
    if hrp.as_str() != HRP_NEVENT {
        return Err(format!("Wrong prefix: expected '{}', got '{}'", HRP_NEVENT, hrp));
    }
    let mut event_id: Option<String> = None;
    let mut relays: Vec<String> = Vec::new();
    let mut author: Option<String> = None;
    let mut pos = 0;
    while pos < bytes.len() {
        if pos + 2 > bytes.len() {
            return Err(String::from("Truncated TLV data"));
        }
        let tlv_type = bytes[pos];
        let tlv_len = bytes[pos + 1] as usize;
        pos += 2;
        if pos + tlv_len > bytes.len() {
            return Err(format!("TLV value overflows buffer: need {} bytes at offset {}, have {}", tlv_len, pos, bytes.len()));
        }
        let tlv_value = &bytes[pos..pos + tlv_len];
        pos += tlv_len;
        match tlv_type {
            TLV_SPECIAL => {
                if tlv_value.len() != 32 {
                    return Err(format!("Invalid event ID length in TLV: expected 32, got {}", tlv_value.len()));
                }
                event_id = Some(bytes_to_hex(tlv_value));
            }
            TLV_RELAY => {
                match std::str::from_utf8(tlv_value) {
                    Ok(url) => relays.push(url.to_string()),
                    Err(_) => {} // skip invalid UTF-8 relay hints
                }
            }
            TLV_AUTHOR => {
                if tlv_value.len() == 32 {
                    author = Some(bytes_to_hex(tlv_value));
                }
            }
            _ => {} // ignore unknown TLV types
        }
    }
    match event_id {
        Some(id) => Ok(DecodedNevent { event_id: id, relays, author }),
        None => Err(String::from("nevent missing required event ID (TLV type 0)")),
    }
}

/// Decode an nprofile1... bech32 string using the NIP-19 TLV format
pub fn decode_nprofile(nprofile: &str) -> Result<DecodedNprofile, String> {
    if !nprofile.starts_with("nprofile1") {
        return Err(String::from("Not an nprofile: must start with 'nprofile1'"));
    }
    let (hrp, bytes) = match bech32::decode(nprofile) {
        Ok(result) => result,
        Err(e) => return Err(format!("Invalid bech32: {}", e)),
    };
    if hrp.as_str() != HRP_NPROFILE {
        return Err(format!("Wrong prefix: expected '{}', got '{}'", HRP_NPROFILE, hrp));
    }
    let mut pubkey: Option<String> = None;
    let mut relays: Vec<String> = Vec::new();
    let mut pos = 0;
    while pos < bytes.len() {
        if pos + 2 > bytes.len() {
            return Err(String::from("Truncated TLV data"));
        }
        let tlv_type = bytes[pos];
        let tlv_len = bytes[pos + 1] as usize;
        pos += 2;
        if pos + tlv_len > bytes.len() {
            return Err(format!("TLV value overflows buffer: need {} bytes at offset {}, have {}", tlv_len, pos, bytes.len()));
        }
        let tlv_value = &bytes[pos..pos + tlv_len];
        pos += tlv_len;
        match tlv_type {
            TLV_SPECIAL => {
                if tlv_value.len() != 32 {
                    return Err(format!("Invalid pubkey length in TLV: expected 32, got {}", tlv_value.len()));
                }
                pubkey = Some(bytes_to_hex(tlv_value));
            }
            TLV_RELAY => {
                match std::str::from_utf8(tlv_value) {
                    Ok(url) => relays.push(url.to_string()),
                    Err(_) => {} // skip invalid UTF-8 relay hints
                }
            }
            _ => {} // ignore unknown TLV types (nprofile doesn't use type 2)
        }
    }
    match pubkey {
        Some(pk) => Ok(DecodedNprofile { pubkey: pk, relays }),
        None => Err(String::from("nprofile missing required pubkey (TLV type 0)")),
    }
}

