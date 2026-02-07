/*
 * messages_store.rs
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

// One file per conversation: ~/.plume/messages/{other_pubkey_hex}.json
// Each file = JSON array of raw kind 4 events (wire format, encrypted content).

use std::fs;
use std::io;
use std::path::Path;

use crate::crypto;
use crate::nostr;

fn messages_dir(config_dir: &str) -> String {
    Path::new(config_dir).join("messages").to_string_lossy().to_string()
}

/// Ensure ~/.plume/messages exists.
pub fn ensure_messages_dir(config_dir: &str) -> Result<(), io::Error> {
    let dir = messages_dir(config_dir);
    let path = Path::new(&dir);
    if path.exists() {
        return Ok(());
    }
    fs::create_dir_all(path)?;
    println!("Created messages directory: {}", path.display());
    Ok(())
}

/// Normalize pubkey to lowercase for consistent filenames.
fn normalize_hex(s: &str) -> String {
    s.trim().to_lowercase()
}

fn conversation_file_path(config_dir: &str, other_pubkey_hex: &str) -> String {
    let other = normalize_hex(other_pubkey_hex);
    Path::new(&messages_dir(config_dir))
        .join(format!("{}.json", other))
        .to_string_lossy()
        .to_string()
}

/// List conversation partner pubkeys (hex) by listing files in messages/.
pub fn list_conversations(config_dir: &str) -> Result<Vec<String>, String> {
    let dir = messages_dir(config_dir);
    let path = Path::new(&dir);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let mut pubkeys: Vec<String> = Vec::new();
    for entry in fs::read_dir(path).map_err(|e| format!("Read messages dir: {}", e))? {
        let entry = entry.map_err(|e| format!("Read dir entry: {}", e))?;
        let name = entry.file_name();
        let name = name.to_str().ok_or("Invalid filename")?;
        if name.ends_with(".json") {
            let pk = name.trim_end_matches(".json");
            if pk.len() == 64 && pk.chars().all(|c| c.is_ascii_hexdigit()) {
                pubkeys.push(pk.to_string());
            }
        }
    }
    Ok(pubkeys)
}

/// One decrypted message for the frontend.
pub struct DecryptedMessage {
    pub id: String,
    pub pubkey: String,
    pub created_at: u64,
    pub content: String,
    pub is_outgoing: bool,
}

/// Read conversation file, decrypt each event, return messages sorted by created_at.
pub fn get_messages(
    config_dir: &str,
    our_secret_hex: &str,
    our_pubkey_hex: &str,
    other_pubkey_hex: &str,
) -> Result<Vec<DecryptedMessage>, String> {
    let path = conversation_file_path(config_dir, other_pubkey_hex);
    let our = normalize_hex(our_pubkey_hex);
    let other = normalize_hex(other_pubkey_hex);

    let contents = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("Read conversation file: {}", e)),
    };

    let parsed = json::parse(&contents).map_err(|e| format!("Invalid JSON: {}", e))?;
    if !parsed.is_array() {
        return Ok(Vec::new());
    }

    let mut messages: Vec<DecryptedMessage> = Vec::new();
    for item in parsed.members() {
        let event_json = item.dump();
        let event = nostr::parse_event(&event_json).map_err(|e| format!("Parse event: {}", e))?;
        if event.kind != nostr::KIND_DM {
            continue;
        }
        let is_outgoing = event.pubkey.to_lowercase() == our;
        let sender_pubkey = if is_outgoing { other.as_str() } else { event.pubkey.as_str() };
        let plaintext = crypto::nip04_decrypt(&event.content, our_secret_hex, sender_pubkey)
            .unwrap_or_else(|_| String::from("[unable to decrypt]"));
        messages.push(DecryptedMessage {
            id: event.id,
            pubkey: event.pubkey,
            created_at: event.created_at,
            content: plaintext,
            is_outgoing,
        });
    }
    messages.sort_by_key(|m| m.created_at);
    Ok(messages)
}

/// Append a raw kind 4 event to the conversation file (dedupe by event id).
pub fn append_raw_event(
    config_dir: &str,
    other_pubkey_hex: &str,
    raw_event_json: &str,
) -> Result<(), String> {
    let path = conversation_file_path(config_dir, other_pubkey_hex);
    let new_event = nostr::parse_event(raw_event_json).map_err(|e| format!("Parse event: {}", e))?;
    if new_event.kind != nostr::KIND_DM {
        return Err(String::from("Event is not kind 4"));
    }

    let mut events: Vec<json::JsonValue> = Vec::new();
    if Path::new(&path).exists() {
        let contents = fs::read_to_string(&path).map_err(|e| format!("Read file: {}", e))?;
        let parsed = json::parse(&contents).map_err(|e| format!("Invalid JSON: {}", e))?;
        if parsed.is_array() {
            for item in parsed.members() {
                events.push(item.clone());
            }
        }
    }

    let new_id = new_event.id.to_lowercase();
    if events.iter().any(|e| e["id"].as_str().map(|s| s.to_lowercase()) == Some(new_id.clone())) {
        return Ok(()); // already present
    }

    let new_obj = json::parse(raw_event_json).map_err(|e| format!("Invalid event JSON: {}", e))?;
    events.push(new_obj);

    let mut out = String::from("[");
    for (i, ev) in events.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&ev.dump());
    }
    out.push(']');

    fs::write(&path, out).map_err(|e| format!("Write file: {}", e))?;
    Ok(())
}

/// Get last event's created_at from a conversation file (no decryption). For sorting the list.
fn last_created_at(config_dir: &str, other_pubkey_hex: &str) -> Option<u64> {
    let path = conversation_file_path(config_dir, other_pubkey_hex);
    let contents = fs::read_to_string(&path).ok()?;
    let parsed = json::parse(&contents).ok()?;
    let last = parsed.members().last()?;
    last["created_at"].as_u64()
}

/// List conversations with last_created_at for sorting. Returns JSON array of { other_pubkey, last_created_at }.
pub fn list_conversations_json(config_dir: &str) -> Result<String, String> {
    let mut list: Vec<(String, u64)> = list_conversations(config_dir)?
        .into_iter()
        .map(|pk| (pk.clone(), last_created_at(config_dir, &pk).unwrap_or(0)))
        .collect();
    list.sort_by(|a, b| b.1.cmp(&a.1));
    let mut out = String::from("[");
    for (i, (pk, ts)) in list.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(r#"{{"other_pubkey":"{}","last_created_at":{}}}"#, escape_json(pk), ts));
    }
    out.push(']');
    Ok(out)
}

fn escape_json(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            _ => o.push(c),
        }
    }
    o
}

/// Serialize decrypted messages to JSON array for the frontend.
pub fn messages_to_json(messages: &[DecryptedMessage]) -> String {
    let mut out = String::from("[");
    for (i, m) in messages.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            r#"{{"id":"{}","pubkey":"{}","created_at":{},"content":"{}","is_outgoing":{}}}"#,
            escape_json(&m.id),
            escape_json(&m.pubkey),
            m.created_at,
            escape_json(&m.content),
            if m.is_outgoing { "true" } else { "false" },
        ));
    }
    out.push(']');
    out
}
