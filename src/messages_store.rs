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

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use bytes::BytesMut;
use crate::crypto;
use crate::debug_log;
use crate::json::{JsonContentHandler, JsonNumber, JsonParser};

/// Per-conversation file lock to prevent concurrent read-modify-write races
/// (e.g. multiple relay DM stream tasks appending the same event).
fn conversation_locks() -> &'static Mutex<HashMap<String, std::sync::Arc<Mutex<()>>>> {
    static INSTANCE: OnceLock<Mutex<HashMap<String, std::sync::Arc<Mutex<()>>>>> = OnceLock::new();
    INSTANCE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_conversation(path: &str) -> std::sync::Arc<Mutex<()>> {
    let mut map = conversation_locks().lock().unwrap();
    map.entry(path.to_string()).or_insert_with(|| std::sync::Arc::new(Mutex::new(()))).clone()
}
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
    debug_log!("Created messages directory: {}", path.display());
    Ok(())
}

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

pub struct DecryptedMessage {
    pub id: String,
    pub pubkey: String,
    pub created_at: u64,
    pub content: String,
    pub is_outgoing: bool,
}

// ============================================================
// Push-parser handler for parsing an array of event objects
// ============================================================

struct EventArrayHandler {
    depth: i32,
    // Event fields (populated while parsing each object)
    current_field: Option<String>,
    event_id: Option<String>,
    event_pubkey: Option<String>,
    event_created_at: u64,
    event_kind: u32,
    event_content: String,
    event_sig: Option<String>,
    event_tags: Vec<Vec<String>>,
    current_tag: Vec<String>,
    tags_depth: i32,
    // Accumulated results
    events: Vec<nostr::Event>,
}

impl EventArrayHandler {
    fn new() -> Self {
        Self {
            depth: 0,
            current_field: None,
            event_id: None,
            event_pubkey: None,
            event_created_at: 0,
            event_kind: 0,
            event_content: String::new(),
            event_sig: None,
            event_tags: Vec::new(),
            current_tag: Vec::new(),
            tags_depth: 0,
            events: Vec::new(),
        }
    }

    fn reset_event(&mut self) {
        self.current_field = None;
        self.event_id = None;
        self.event_pubkey = None;
        self.event_created_at = 0;
        self.event_kind = 0;
        self.event_content.clear();
        self.event_sig = None;
        self.event_tags.clear();
        self.current_tag.clear();
        self.tags_depth = 0;
    }
}

impl JsonContentHandler for EventArrayHandler {
    fn start_object(&mut self) {
        self.depth += 1;
        if self.depth == 2 {
            self.reset_event();
        }
    }

    fn end_object(&mut self) {
        if self.depth == 2 {
            // Finished one event object
            if let (Some(id), Some(pubkey), Some(sig)) = 
                (self.event_id.clone(), self.event_pubkey.clone(), self.event_sig.clone()) 
            {
                self.events.push(nostr::Event {
                    id,
                    pubkey,
                    created_at: self.event_created_at,
                    kind: self.event_kind,
                    tags: self.event_tags.clone(),
                    content: self.event_content.clone(),
                    sig,
                });
            }
        }
        self.depth -= 1;
    }

    fn start_array(&mut self) {
        self.depth += 1;
        if self.tags_depth == 1 {
            self.tags_depth = 2;
            self.current_tag.clear();
        } else if self.tags_depth == 2 {
            self.current_tag.clear();
        }
    }

    fn end_array(&mut self) {
        if self.tags_depth == 2 && self.depth == 4 {
            if !self.current_tag.is_empty() {
                self.event_tags.push(self.current_tag.clone());
            }
            self.current_tag.clear();
        } else if self.tags_depth == 2 && self.depth == 3 {
            self.tags_depth = 0;
        } else if self.tags_depth == 1 && self.depth == 3 {
            self.tags_depth = 0;
        }
        self.depth -= 1;
    }

    fn key(&mut self, key: &str) {
        self.current_field = Some(key.to_string());
        if self.depth == 2 && key == "tags" {
            self.tags_depth = 1;
        }
    }

    fn string_value(&mut self, value: &str) {
        if self.tags_depth == 2 {
            self.current_tag.push(value.to_string());
        } else if self.depth == 2 {
            if let Some(ref f) = self.current_field {
                match f.as_str() {
                    "id" => self.event_id = Some(value.to_string()),
                    "pubkey" => self.event_pubkey = Some(value.to_string()),
                    "content" => self.event_content = value.to_string(),
                    "sig" => self.event_sig = Some(value.to_string()),
                    _ => {}
                }
            }
        }
    }

    fn number_value(&mut self, number: JsonNumber) {
        if self.depth == 2 {
            if let Some(ref f) = self.current_field {
                if f == "created_at" {
                    self.event_created_at = number.as_f64().max(0.0) as u64;
                } else if f == "kind" {
                    self.event_kind = number.as_f64().max(0.0) as u32;
                }
            }
        }
    }

    fn boolean_value(&mut self, _value: bool) {}
    fn null_value(&mut self) {}
}

/// Parse a JSON array of event objects from a string.
fn parse_event_array(json_str: &str) -> Result<Vec<nostr::Event>, String> {
    let mut handler = EventArrayHandler::new();
    let mut parser = JsonParser::new();
    let mut buf = BytesMut::from(json_str.as_bytes());
    parser.receive(&mut buf, &mut handler).map_err(|e| format!("JSON parse error: {}", e))?;
    parser.close(&mut handler).map_err(|e| format!("JSON parse error: {}", e))?;
    Ok(handler.events)
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

    let events = parse_event_array(&contents)?;

    let mut messages: Vec<DecryptedMessage> = Vec::new();
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for event in &events {
        if event.kind != nostr::KIND_DM {
            continue;
        }
        // Deduplicate by event ID (safety net for any races that wrote dupes)
        let id_lower = event.id.to_lowercase();
        if !seen_ids.insert(id_lower) {
            continue;
        }
        let is_outgoing = event.pubkey.to_lowercase() == our;
        let sender_pubkey = if is_outgoing { other.as_str() } else { event.pubkey.as_str() };
        let plaintext = crypto::nip04_decrypt(&event.content, our_secret_hex, sender_pubkey)
            .unwrap_or_else(|_| String::from("[unable to decrypt]"));
        messages.push(DecryptedMessage {
            id: event.id.clone(),
            pubkey: event.pubkey.clone(),
            created_at: event.created_at,
            content: plaintext,
            is_outgoing,
        });
    }
    messages.sort_by_key(|m| m.created_at);
    Ok(messages)
}

/// Append a raw kind 4 event to the conversation file (dedupe by event id).
/// Returns Ok(true) if the event was actually appended, Ok(false) if duplicate.
pub fn append_raw_event(
    config_dir: &str,
    other_pubkey_hex: &str,
    raw_event_json: &str,
) -> Result<bool, String> {
    let path = conversation_file_path(config_dir, other_pubkey_hex);
    let new_event = nostr::parse_event(raw_event_json).map_err(|e| format!("Parse event: {}", e))?;
    if new_event.kind != nostr::KIND_DM {
        return Err(String::from("Event is not kind 4"));
    }

    let new_id = new_event.id.to_lowercase();

    // Hold a per-conversation lock for the entire read-check-write cycle
    // to prevent concurrent relay stream tasks from duplicating events.
    let lock = lock_conversation(&path);
    let _guard = lock.lock().unwrap();

    if Path::new(&path).exists() {
        let contents = fs::read_to_string(&path).map_err(|e| format!("Read file: {}", e))?;
        // Dedup: search for the event ID in the raw file text
        let search_pattern = format!("\"id\":\"{}\"", new_id);
        if contents.to_lowercase().contains(&search_pattern) {
            return Ok(false); // already present — duplicate
        }
        // Append: strip trailing ] and add ,event]
        let trimmed = contents.trim_end();
        if trimmed.ends_with(']') {
            let mut out = String::from(&trimmed[..trimmed.len() - 1]);
            if out.trim_end().ends_with('}') {
                out.push(',');
            }
            out.push_str(raw_event_json);
            out.push(']');
            fs::write(&path, out).map_err(|e| format!("Write file: {}", e))?;
        } else {
            // File is malformed, rewrite
            let mut out = String::from("[");
            out.push_str(raw_event_json);
            out.push(']');
            fs::write(&path, out).map_err(|e| format!("Write file: {}", e))?;
        }
    } else {
        // New file
        let out = format!("[{}]", raw_event_json);
        fs::write(&path, out).map_err(|e| format!("Write file: {}", e))?;
    }

    Ok(true) // genuinely new event
}

/// Get last event's created_at from a conversation file. Scans for the last "created_at": number.
fn last_created_at(config_dir: &str, other_pubkey_hex: &str) -> Option<u64> {
    let path = conversation_file_path(config_dir, other_pubkey_hex);
    let contents = fs::read_to_string(&path).ok()?;
    // Find last occurrence of "created_at": followed by digits
    let pattern = "\"created_at\":";
    let pos = contents.rfind(pattern)?;
    let after = &contents[pos + pattern.len()..];
    let after = after.trim_start();
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u64>().ok()
}

/// Count conversations with messages newer than `since` (unix timestamp).
/// Returns the number of conversations that have at least one event with
/// created_at > since.  This gives a "conversations with unread" count.
pub fn count_unread_conversations(config_dir: &str, since: u64) -> u32 {
    let convos = match list_conversations(config_dir) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let mut count = 0u32;
    for pk in &convos {
        if let Some(ts) = last_created_at(config_dir, pk) {
            if ts > since {
                count += 1;
            }
        }
    }
    count
}

/// List conversations with last_created_at for sorting.
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
