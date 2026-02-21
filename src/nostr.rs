/*
 * nostr.rs
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

use bytes::BytesMut;
use crate::json::{JsonContentHandler, JsonNumber, JsonParser};

// A Nostr event - the fundamental data structure in Nostr
// See: https://github.com/nostr-protocol/nips/blob/master/01.md
pub struct Event {
    // Unique identifier (32-byte hex, SHA256 of serialized event)
    pub id: String,
    
    // Public key of the event creator (32-byte hex)
    pub pubkey: String,
    
    // Unix timestamp when the event was created
    pub created_at: u64,
    
    // Event kind (1 = text note, 0 = metadata, etc.)
    pub kind: u32,
    
    // Array of tags (each tag is an array of strings)
    pub tags: Vec<Vec<String>>,
    
    // The actual content of the event
    pub content: String,
    
    // Signature of the event (64-byte hex)
    pub sig: String,
}

// Common event kinds in Nostr
pub const KIND_METADATA: u32 = 0;       // User profile metadata
pub const KIND_TEXT_NOTE: u32 = 1;      // Short text note (like a tweet)
#[allow(dead_code)]
pub const KIND_RECOMMEND_RELAY: u32 = 2; // Relay recommendation
pub const KIND_CONTACTS: u32 = 3;       // Contact list / follows
/// NIP-04: Encrypted direct message
pub const KIND_DM: u32 = 4;
#[allow(dead_code)]
pub const KIND_REPOST: u32 = 6;         // Repost/boost of another note
#[allow(dead_code)]
pub const KIND_REACTION: u32 = 7;       // Reaction (like, emoji)
/// NIP-59: Seal (encrypted rumor, signed by sender)
pub const KIND_SEAL: u32 = 13;
/// NIP-17: Private chat message (rumor kind inside seal/gift wrap)
pub const KIND_CHAT_MESSAGE: u32 = 14;
/// NIP-59: Gift wrap (encrypted seal, signed by ephemeral key)
pub const KIND_GIFT_WRAP: u32 = 1059;
pub const KIND_ZAP_REQUEST: u32 = 9734; // NIP-57 Lightning zap request
pub const KIND_ZAP_RECEIPT: u32 = 9735; // NIP-57 Lightning zap receipt
/// Blossom auth event (BUD-01): tags ["t", action], ["x", sha256], ["expiration", ts]
pub const KIND_BLOSSOM_AUTH: u32 = 24242;
/// NIP-98 HTTP Auth event: tags ["u", url], ["method", method]
pub const KIND_HTTP_AUTH: u32 = 27235;
/// NIP-17: DM relay list (tags: ["relay", "wss://..."])
pub const KIND_DM_RELAY_LIST: u32 = 10050;
pub const KIND_LONG_FORM: u32 = 30023;  // NIP-23 Long-form content (articles)
/// NIP-65: Relay list metadata (tags: ["r", "relay_url"] or ["r", "url", "read"/"write"])
pub const KIND_RELAY_LIST: u32 = 10002;

// A filter for requesting events from relays
#[derive(Clone, Default)]
pub struct Filter {
    // Filter by event IDs
    pub ids: Option<Vec<String>>,
    
    // Filter by author public keys
    pub authors: Option<Vec<String>>,
    
    // Filter by event kinds
    pub kinds: Option<Vec<u32>>,
    
    // Filter by events created after this timestamp
    pub since: Option<u64>,
    
    // Filter by events created before this timestamp
    pub until: Option<u64>,
    
    // Maximum number of events to return
    pub limit: Option<u32>,
    
    // Filter by "p" tags (pubkeys referenced in events)
    // This is used for finding followers (kind 3 events that tag a pubkey)
    pub p_tags: Option<Vec<String>>,

    // Filter by "e" tags (event IDs referenced, e.g. replies to an event). NIP-01 #e.
    pub e_tags: Option<Vec<String>>,

    // Filter by "t" tags (hashtags). NIP-01 #t.
    pub t_tags: Option<Vec<String>>,

    // NIP-50 full-text search term.
    pub search: Option<String>,
}

// Create a new empty filter
impl Filter {
    #[allow(dead_code)]
    pub fn new() -> Filter {
        Filter {
            ids: None,
            authors: None,
            kinds: None,
            since: None,
            until: None,
            limit: None,
            p_tags: None,
            e_tags: None,
            t_tags: None,
            search: None,
        }
    }
}

// User profile metadata (kind 0 event content)
pub struct ProfileMetadata {
    pub name: Option<String>,
    pub about: Option<String>,
    pub picture: Option<String>,
    pub nip05: Option<String>,
    pub banner: Option<String>,
    pub website: Option<String>,
    pub lud16: Option<String>,  // Lightning address
    /// When the profile (kind 0) event was created; from event.created_at
    pub created_at: Option<u64>,
}

impl ProfileMetadata {
    #[allow(dead_code)]
    pub fn new() -> ProfileMetadata {
        ProfileMetadata {
            name: None,
            about: None,
            picture: None,
            nip05: None,
            banner: None,
            website: None,
            lud16: None,
            created_at: None,
        }
    }
}

// ============================================================
// JSON Push-Parser Handlers
// ============================================================

/// Handler for parsing a single Nostr Event from JSON.
struct EventHandler {
    depth: i32,
    current_field: Option<String>,
    id: Option<String>,
    pubkey: Option<String>,
    created_at: u64,
    kind: u32,
    content: String,
    sig: Option<String>,
    tags: Vec<Vec<String>>,
    current_tag: Vec<String>,
    tags_depth: i32, // 0=not in tags, 1=in tags array, 2=in one tag array
}

impl EventHandler {
    fn new() -> Self {
        Self {
            depth: 0,
            current_field: None,
            id: None,
            pubkey: None,
            created_at: 0,
            kind: 0,
            content: String::new(),
            sig: None,
            tags: Vec::new(),
            current_tag: Vec::new(),
            tags_depth: 0,
        }
    }

    fn take_event(&self) -> Result<Event, String> {
        Ok(Event {
            id: self.id.clone().ok_or("Missing 'id' field")?,
            pubkey: self.pubkey.clone().ok_or("Missing 'pubkey' field")?,
            created_at: self.created_at,
            kind: self.kind,
            tags: self.tags.clone(),
            content: self.content.clone(),
            sig: self.sig.clone().unwrap_or_default(),
        })
    }
}

impl JsonContentHandler for EventHandler {
    fn start_object(&mut self) {
        self.depth += 1;
    }

    fn end_object(&mut self) {
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
        if self.tags_depth == 2 && self.depth == 3 {
            if !self.current_tag.is_empty() {
                self.tags.push(self.current_tag.clone());
            }
            self.current_tag.clear();
        } else if self.tags_depth == 2 && self.depth == 2 {
            self.tags_depth = 0;
        } else if self.tags_depth == 1 && self.depth == 2 {
            self.tags_depth = 0;
        }
        self.depth -= 1;
    }

    fn key(&mut self, key: &str) {
        self.current_field = Some(key.to_string());
        if self.depth == 1 && key == "tags" {
            self.tags_depth = 1;
        }
    }

    fn string_value(&mut self, value: &str) {
        if self.tags_depth == 2 {
            self.current_tag.push(value.to_string());
        } else if self.depth == 1 {
            if let Some(ref f) = self.current_field {
                match f.as_str() {
                    "id" => self.id = Some(value.to_string()),
                    "pubkey" => self.pubkey = Some(value.to_string()),
                    "content" => self.content = value.to_string(),
                    "sig" => self.sig = Some(value.to_string()),
                    _ => {}
                }
            }
        }
    }

    fn number_value(&mut self, number: JsonNumber) {
        if self.depth == 1 {
            if let Some(ref f) = self.current_field {
                if f == "created_at" {
                    self.created_at = number.as_f64().max(0.0) as u64;
                } else if f == "kind" {
                    self.kind = number.as_f64().max(0.0) as u32;
                }
            }
        }
    }

    fn boolean_value(&mut self, _value: bool) {}
    fn null_value(&mut self) {}
}

/// Handler for parsing ProfileMetadata from JSON.
struct ProfileHandler {
    current_field: Option<String>,
    name: Option<String>,
    about: Option<String>,
    picture: Option<String>,
    nip05: Option<String>,
    banner: Option<String>,
    website: Option<String>,
    lud16: Option<String>,
}

impl ProfileHandler {
    fn new() -> Self {
        Self {
            current_field: None,
            name: None,
            about: None,
            picture: None,
            nip05: None,
            banner: None,
            website: None,
            lud16: None,
        }
    }

    fn take_profile(&self) -> ProfileMetadata {
        ProfileMetadata {
            name: self.name.clone(),
            about: self.about.clone(),
            picture: self.picture.clone(),
            nip05: self.nip05.clone(),
            banner: self.banner.clone(),
            website: self.website.clone(),
            lud16: self.lud16.clone(),
            created_at: None,
        }
    }
}

impl JsonContentHandler for ProfileHandler {
    fn start_object(&mut self) {}
    fn end_object(&mut self) {}
    fn start_array(&mut self) {}
    fn end_array(&mut self) {}

    fn key(&mut self, key: &str) {
        self.current_field = Some(key.to_string());
    }

    fn string_value(&mut self, value: &str) {
        if let Some(ref f) = self.current_field {
            match f.as_str() {
                "name" => self.name = Some(value.to_string()),
                "about" => self.about = Some(value.to_string()),
                "picture" => self.picture = Some(value.to_string()),
                "nip05" => self.nip05 = Some(value.to_string()),
                "banner" => self.banner = Some(value.to_string()),
                "website" => self.website = Some(value.to_string()),
                "lud16" => self.lud16 = Some(value.to_string()),
                _ => {}
            }
        }
    }

    fn number_value(&mut self, _number: JsonNumber) {}
    fn boolean_value(&mut self, _value: bool) {}
    fn null_value(&mut self) {}
}

// ============================================================
// JSON Parsing Functions (using push parser)
// ============================================================

/// Helper: run the push parser on a complete JSON string, calling handler.
fn parse_json_str<H: JsonContentHandler>(json_str: &str, handler: &mut H) -> Result<(), String> {
    let mut parser = JsonParser::new();
    let mut buf = BytesMut::from(json_str.as_bytes());
    parser.receive(&mut buf, handler).map_err(|e| format!("JSON parse error: {}", e))?;
    parser.close(handler).map_err(|e| format!("JSON parse error: {}", e))?;
    Ok(())
}

// Parse a JSON string into a Nostr Event
pub fn parse_event(json_str: &str) -> Result<Event, String> {
    let mut handler = EventHandler::new();
    parse_json_str(json_str, &mut handler)?;
    handler.take_event()
}

// Parse profile metadata from a kind 0 event's content
pub fn parse_profile(content: &str) -> Result<ProfileMetadata, String> {
    let mut handler = ProfileHandler::new();
    parse_json_str(content, &mut handler)?;
    Ok(handler.take_profile())
}

// ============================================================
// JSON Serialization Functions (manual string building, no crate)
// ============================================================

// Convert an Event to JSON string
pub fn event_to_json(event: &Event) -> String {
    let mut json = String::new();
    json.push_str("{\n");
    
    // id
    json.push_str("  \"id\": \"");
    json.push_str(&escape_json_string(&event.id));
    json.push_str("\",\n");
    
    // pubkey
    json.push_str("  \"pubkey\": \"");
    json.push_str(&escape_json_string(&event.pubkey));
    json.push_str("\",\n");
    
    // created_at
    json.push_str("  \"created_at\": ");
    json.push_str(&event.created_at.to_string());
    json.push_str(",\n");
    
    // kind
    json.push_str("  \"kind\": ");
    json.push_str(&event.kind.to_string());
    json.push_str(",\n");
    
    // tags
    json.push_str("  \"tags\": [");
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
    json.push_str("],\n");
    
    // content
    json.push_str("  \"content\": \"");
    json.push_str(&escape_json_string(&event.content));
    json.push_str("\",\n");
    
    // sig
    json.push_str("  \"sig\": \"");
    json.push_str(&escape_json_string(&event.sig));
    json.push_str("\"\n");
    
    json.push_str("}");
    
    return json;
}

// Convert a Filter to JSON string (for REQ messages to relays)
pub fn filter_to_json(filter: &Filter) -> String {
    let mut json = String::new();
    json.push_str("{");
    
    let mut first = true;
    
    // ids
    if let Some(ref ids) = filter.ids {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"ids\":[");
        for (i, id) in ids.iter().enumerate() {
            json.push_str("\"");
            json.push_str(&escape_json_string(id));
            json.push_str("\"");
            if i < ids.len() - 1 {
                json.push_str(",");
            }
        }
        json.push_str("]");
    }
    
    // authors
    if let Some(ref authors) = filter.authors {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"authors\":[");
        for (i, author) in authors.iter().enumerate() {
            json.push_str("\"");
            json.push_str(&escape_json_string(author));
            json.push_str("\"");
            if i < authors.len() - 1 {
                json.push_str(",");
            }
        }
        json.push_str("]");
    }
    
    // kinds
    if let Some(ref kinds) = filter.kinds {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"kinds\":[");
        for (i, kind) in kinds.iter().enumerate() {
            json.push_str(&kind.to_string());
            if i < kinds.len() - 1 {
                json.push_str(",");
            }
        }
        json.push_str("]");
    }
    
    // since
    if let Some(since) = filter.since {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"since\":");
        json.push_str(&since.to_string());
    }
    
    // until
    if let Some(until) = filter.until {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"until\":");
        json.push_str(&until.to_string());
    }
    
    // limit
    if let Some(limit) = filter.limit {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"limit\":");
        json.push_str(&limit.to_string());
    }
    
    // #p tags (for filtering by referenced pubkeys)
    if let Some(ref p_tags) = filter.p_tags {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"#p\":[");
        for (i, pubkey) in p_tags.iter().enumerate() {
            json.push_str("\"");
            json.push_str(&escape_json_string(pubkey));
            json.push_str("\"");
            if i < p_tags.len() - 1 {
                json.push_str(",");
            }
        }
        json.push_str("]");
    }

    // #e tags (for filtering by referenced event IDs, e.g. replies)
    if let Some(ref e_tags) = filter.e_tags {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"#e\":[");
        for (i, eid) in e_tags.iter().enumerate() {
            json.push_str("\"");
            json.push_str(&escape_json_string(eid));
            json.push_str("\"");
            if i < e_tags.len() - 1 {
                json.push_str(",");
            }
        }
        json.push_str("]");
    }

    // #t tags (for filtering by hashtags)
    if let Some(ref t_tags) = filter.t_tags {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"#t\":[");
        for (i, tag) in t_tags.iter().enumerate() {
            json.push_str("\"");
            json.push_str(&escape_json_string(tag));
            json.push_str("\"");
            if i < t_tags.len() - 1 {
                json.push_str(",");
            }
        }
        json.push_str("]");
    }

    // NIP-50 search term
    if let Some(ref search) = filter.search {
        if !first {
            json.push_str(",");
        }
        let _ = first;
        json.push_str("\"search\":\"");
        json.push_str(&escape_json_string(search));
        json.push_str("\"");
    }
    
    json.push_str("}");
    
    return json;
}

// Escape special characters in a string for JSON
fn escape_json_string(input: &str) -> String {
    let mut output = String::new();
    
    for character in input.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            _ => output.push(character),
        }
    }
    
    return output;
}

// ============================================================
// Helper Functions
// ============================================================

/// Create a filter for text notes from specific authors (optionally only since timestamp).
#[allow(dead_code)]
pub fn filter_notes_by_authors(authors: Vec<String>, limit: u32) -> Filter {
    filter_notes_by_authors_since(authors, limit, None)
}

pub fn filter_notes_by_authors_since(authors: Vec<String>, limit: u32, since: Option<u64>) -> Filter {
    Filter {
        authors: Some(authors),
        kinds: Some(vec![KIND_TEXT_NOTE, KIND_LONG_FORM]),
        since,
        limit: Some(limit),
        ..Default::default()
    }
}

/// Profile feed: notes (kind 1), reposts (kind 6), and articles (kind 30023) by authors.
pub fn filter_profile_feed_by_authors_since(authors: Vec<String>, limit: u32, since: Option<u64>) -> Filter {
    Filter {
        authors: Some(authors),
        kinds: Some(vec![KIND_TEXT_NOTE, KIND_REPOST, KIND_LONG_FORM]),
        since,
        limit: Some(limit),
        ..Default::default()
    }
}

// Create a filter for recent global notes (optionally only since timestamp)
pub fn filter_recent_notes(limit: u32) -> Filter {
    filter_recent_notes_since(limit, None)
}

pub fn filter_recent_notes_since(limit: u32, since: Option<u64>) -> Filter {
    Filter {
        kinds: Some(vec![KIND_TEXT_NOTE, KIND_LONG_FORM]),
        since,
        limit: Some(limit),
        ..Default::default()
    }
}

/// Create a filter to fetch kind 1 notes that reference the given event ID in an "e" tag (replies).
pub fn filter_replies_to_event(event_id: String, limit: u32) -> Filter {
    Filter {
        kinds: Some(vec![KIND_TEXT_NOTE]),
        limit: Some(limit),
        e_tags: Some(vec![event_id]),
        ..Default::default()
    }
}

/// Get the recipient pubkey (hex) from a kind 4 event's "p" tag. Returns None if missing or not kind 4.
pub fn get_recipient_pubkey_from_kind4(event: &Event) -> Option<String> {
    if event.kind != KIND_DM {
        return None;
    }
    for tag in &event.tags {
        if tag.len() >= 2 && tag[0] == "p" {
            return Some(tag[1].clone());
        }
    }
    None
}

/// For a kind 4 event, the "other" party (conversation partner) is the one that is not us.
pub fn other_pubkey_in_dm(event: &Event, our_pubkey_hex: &str) -> Option<String> {
    let our = our_pubkey_hex.to_lowercase();
    let sender = event.pubkey.to_lowercase();
    let recipient = get_recipient_pubkey_from_kind4(event)?.to_lowercase();
    if sender == our {
        Some(recipient)
    } else if recipient == our {
        Some(sender)
    } else {
        None
    }
}

/// Filter for DMs we received: kind 4 with #p = our pubkey.
pub fn filter_dms_received(our_pubkey_hex: &str, limit: u32, since: Option<u64>) -> Filter {
    Filter {
        kinds: Some(vec![KIND_DM]),
        since,
        limit: Some(limit),
        p_tags: Some(vec![our_pubkey_hex.to_string()]),
        ..Default::default()
    }
}

/// Filter for DMs we sent: kind 4 with authors = our pubkey.
pub fn filter_dms_sent(our_pubkey_hex: &str, limit: u32, since: Option<u64>) -> Filter {
    Filter {
        authors: Some(vec![our_pubkey_hex.to_string()]),
        kinds: Some(vec![KIND_DM]),
        since,
        limit: Some(limit),
        ..Default::default()
    }
}

/// Create a filter to fetch events by their IDs (e.g. for bookmarks).
pub fn filter_events_by_ids(ids: Vec<String>) -> Filter {
    Filter {
        ids: Some(ids),
        kinds: Some(vec![KIND_TEXT_NOTE, KIND_LONG_FORM]),
        ..Default::default()
    }
}

/// Filter for zap receipts (kind 9735) tagged with a pubkey.
pub fn filter_zap_receipts_by_pubkey(pubkey: &str, limit: u32) -> Filter {
    Filter {
        kinds: Some(vec![KIND_ZAP_RECEIPT]),
        limit: Some(limit),
        p_tags: Some(vec![pubkey.to_string()]),
        ..Default::default()
    }
}

/// Filter for zap receipts (kind 9735) tagged with specific event IDs.
pub fn filter_zap_receipts_by_events(event_ids: Vec<String>, limit: u32) -> Filter {
    Filter {
        kinds: Some(vec![KIND_ZAP_RECEIPT]),
        limit: Some(limit),
        e_tags: Some(event_ids),
        ..Default::default()
    }
}

/// Parsed info from a zap receipt (kind 9735).
pub struct ZapReceiptInfo {
    pub receipt_id: String,
    pub created_at: u64,
    pub sender_pubkey: String,
    pub recipient_pubkey: String,
    pub amount_msats: u64,
    pub zapped_event_id: Option<String>,
    pub message: String,
}

/// Extract zap info from a kind 9735 receipt event.
/// The `description` tag contains the JSON of the original kind 9734 zap request.
pub fn parse_zap_receipt(event: &Event) -> Option<ZapReceiptInfo> {
    if event.kind != KIND_ZAP_RECEIPT {
        return None;
    }
    let recipient = event.tags.iter()
        .find(|t| t.len() >= 2 && t[0] == "p")
        .map(|t| t[1].clone())?;
    let zapped_event = event.tags.iter()
        .find(|t| t.len() >= 2 && t[0] == "e")
        .map(|t| t[1].clone());
    let bolt11 = event.tags.iter()
        .find(|t| t.len() >= 2 && t[0] == "bolt11")
        .map(|t| t[1].clone());
    let description_json = event.tags.iter()
        .find(|t| t.len() >= 2 && t[0] == "description")
        .map(|t| t[1].clone())?;

    let zap_request = parse_event(&description_json).ok()?;
    if zap_request.kind != KIND_ZAP_REQUEST {
        return None;
    }

    let sender = zap_request.pubkey.clone();
    let amount_msats = zap_request.tags.iter()
        .find(|t| t.len() >= 2 && t[0] == "amount")
        .and_then(|t| t[1].parse::<u64>().ok())
        .or_else(|| bolt11.as_deref().and_then(parse_bolt11_amount_msats))
        .unwrap_or(0);

    Some(ZapReceiptInfo {
        receipt_id: event.id.clone(),
        created_at: event.created_at,
        sender_pubkey: sender,
        recipient_pubkey: recipient,
        amount_msats,
        zapped_event_id: zapped_event,
        message: zap_request.content.clone(),
    })
}

/// Parse amount in millisats from a bolt11 invoice string.
fn parse_bolt11_amount_msats(invoice: &str) -> Option<u64> {
    let lower = invoice.to_lowercase();
    let prefix = if lower.starts_with("lnbc") {
        &lower[4..]
    } else {
        return None;
    };
    let sep = prefix.find('1')?;
    let amount_part = &prefix[..sep];
    if amount_part.is_empty() {
        return None;
    }
    let multiplier_char = amount_part.chars().last()?;
    let (digits, mult_msats) = match multiplier_char {
        'm' => (&amount_part[..amount_part.len() - 1], 100_000_000u64),
        'u' => (&amount_part[..amount_part.len() - 1], 100_000u64),
        'n' => (&amount_part[..amount_part.len() - 1], 100u64),
        'p' => (&amount_part[..amount_part.len() - 1], 1u64), // 0.1 msat, rounds to 1
        '0'..='9' => (amount_part, 100_000_000_000u64), // bare BTC amount
        _ => return None,
    };
    let num: u64 = digits.parse().ok()?;
    Some(num * mult_msats)
}

pub fn zap_receipts_to_json(receipts: &[ZapReceiptInfo]) -> String {
    let mut out = String::from("[");
    for (i, z) in receipts.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            r#"{{"receipt_id":"{}","created_at":{},"sender_pubkey":"{}","recipient_pubkey":"{}","amount_sats":{},"zapped_event_id":{},"message":"{}"}}"#,
            escape_json_string(&z.receipt_id),
            z.created_at,
            escape_json_string(&z.sender_pubkey),
            escape_json_string(&z.recipient_pubkey),
            z.amount_msats / 1000,
            match &z.zapped_event_id {
                Some(id) => format!("\"{}\"", escape_json_string(id)),
                None => String::from("null"),
            },
            escape_json_string(&z.message),
        ));
    }
    out.push(']');
    out
}

/// Build a JSON object mapping event_id -> total_sats from a list of zap receipts.
pub fn zap_totals_to_json(receipts: &[ZapReceiptInfo]) -> String {
    let mut totals: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    for z in receipts {
        if let Some(ref eid) = z.zapped_event_id {
            *totals.entry(eid.clone()).or_insert(0) += z.amount_msats / 1000;
        }
    }
    let mut out = String::from("{");
    for (i, (eid, total)) in totals.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!("\"{}\":{}", escape_json_string(eid), total));
    }
    out.push('}');
    out
}

// Create a filter for profile metadata (kind 0) by author
pub fn filter_profile_by_author(author_pubkey: &str) -> Filter {
    Filter {
        authors: Some(vec![author_pubkey.to_string()]),
        kinds: Some(vec![KIND_METADATA]),
        limit: Some(1),
        ..Default::default()
    }
}

/// Create a filter for multiple profiles at once.
#[allow(dead_code)]
pub fn filter_profiles_by_authors(author_pubkeys: Vec<String>) -> Filter {
    Filter {
        authors: Some(author_pubkeys),
        kinds: Some(vec![KIND_METADATA]),
        ..Default::default()
    }
}

// Convert a ProfileMetadata to JSON string
pub fn profile_to_json(profile: &ProfileMetadata) -> String {
    let mut json = String::new();
    json.push_str("{");
    
    let mut first = true;
    
    if let Some(ref name) = profile.name {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"name\":\"");
        json.push_str(&escape_json_string(name));
        json.push_str("\"");
    }
    if let Some(ref about) = profile.about {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"about\":\"");
        json.push_str(&escape_json_string(about));
        json.push_str("\"");
    }
    if let Some(ref picture) = profile.picture {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"picture\":\"");
        json.push_str(&escape_json_string(picture));
        json.push_str("\"");
    }
    if let Some(ref nip05) = profile.nip05 {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"nip05\":\"");
        json.push_str(&escape_json_string(nip05));
        json.push_str("\"");
    }
    if let Some(ref banner) = profile.banner {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"banner\":\"");
        json.push_str(&escape_json_string(banner));
        json.push_str("\"");
    }
    if let Some(ref website) = profile.website {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"website\":\"");
        json.push_str(&escape_json_string(website));
        json.push_str("\"");
    }
    if let Some(ref lud16) = profile.lud16 {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"lud16\":\"");
        json.push_str(&escape_json_string(lud16));
        json.push_str("\"");
    }
    if let Some(created_at) = profile.created_at {
        if !first {
            json.push_str(",");
        }
        let _ = first;
        json.push_str("\"created_at\":");
        json.push_str(&created_at.to_string());
    }
    
    json.push_str("}");
    return json;
}

/// Build kind 0 event content JSON (profile fields only; no created_at - that is the event timestamp).
pub fn profile_to_content(profile: &ProfileMetadata) -> String {
    let mut json = String::new();
    json.push_str("{");
    let mut first = true;
    if let Some(ref name) = profile.name {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"name\":\"");
        json.push_str(&escape_json_string(name));
        json.push_str("\"");
    }
    if let Some(ref about) = profile.about {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"about\":\"");
        json.push_str(&escape_json_string(about));
        json.push_str("\"");
    }
    if let Some(ref picture) = profile.picture {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"picture\":\"");
        json.push_str(&escape_json_string(picture));
        json.push_str("\"");
    }
    if let Some(ref nip05) = profile.nip05 {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"nip05\":\"");
        json.push_str(&escape_json_string(nip05));
        json.push_str("\"");
    }
    if let Some(ref banner) = profile.banner {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"banner\":\"");
        json.push_str(&escape_json_string(banner));
        json.push_str("\"");
    }
    if let Some(ref website) = profile.website {
        if !first {
            json.push_str(",");
        }
        first = false;
        json.push_str("\"website\":\"");
        json.push_str(&escape_json_string(website));
        json.push_str("\"");
    }
    if let Some(ref lud16) = profile.lud16 {
        if !first {
            json.push_str(",");
        }
        let _ = first;
        json.push_str("\"lud16\":\"");
        json.push_str(&escape_json_string(lud16));
        json.push_str("\"");
    }
    json.push_str("}");
    json
}

// ============================================================
// Contact List (Following/Followers)
// ============================================================

pub struct Contact {
    pub pubkey: String,
    pub relay_url: Option<String>,
    pub petname: Option<String>,
}

pub struct ContactList {
    pub owner_pubkey: String,
    pub contacts: Vec<Contact>,
    pub created_at: u64,
}

pub fn parse_contact_list(event: &Event) -> Result<ContactList, String> {
    if event.kind != KIND_CONTACTS {
        return Err(format!("Expected kind 3 event, got kind {}", event.kind));
    }
    
    let mut contacts: Vec<Contact> = Vec::new();
    
    for tag in &event.tags {
        if tag.len() >= 2 && tag[0] == "p" {
            let pubkey = tag[1].clone();
            let relay_url = if tag.len() >= 3 && !tag[2].is_empty() {
                Some(tag[2].clone())
            } else {
                None
            };
            let petname = if tag.len() >= 4 && !tag[3].is_empty() {
                Some(tag[3].clone())
            } else {
                None
            };
            contacts.push(Contact { pubkey, relay_url, petname });
        }
    }
    
    return Ok(ContactList {
        owner_pubkey: event.pubkey.clone(),
        contacts,
        created_at: event.created_at,
    });
}

#[allow(dead_code)]
pub fn get_following_pubkeys(contact_list: &ContactList) -> Vec<String> {
    contact_list.contacts.iter().map(|c| c.pubkey.clone()).collect()
}

pub fn filter_contact_list_by_author(author_pubkey: &str) -> Filter {
    Filter {
        authors: Some(vec![author_pubkey.to_string()]),
        kinds: Some(vec![KIND_CONTACTS]),
        limit: Some(1),
        ..Default::default()
    }
}

pub fn filter_followers_by_pubkey(target_pubkey: &str) -> Filter {
    Filter {
        kinds: Some(vec![KIND_CONTACTS]),
        limit: Some(500),
        p_tags: Some(vec![target_pubkey.to_string()]),
        ..Default::default()
    }
}

pub fn filter_relay_list_by_author(author_pubkey: &str) -> Filter {
    Filter {
        authors: Some(vec![author_pubkey.to_string()]),
        kinds: Some(vec![KIND_RELAY_LIST]),
        limit: Some(1),
        ..Default::default()
    }
}

pub fn parse_relay_list(event: &Event) -> Result<Vec<String>, String> {
    if event.kind != KIND_RELAY_LIST {
        return Err(format!("Expected kind 10002 event, got kind {}", event.kind));
    }
    let mut urls: Vec<String> = Vec::new();
    for tag in &event.tags {
        if tag.len() >= 2 && tag[0] == "r" && !tag[1].is_empty() {
            let url = tag[1].trim();
            if !url.is_empty() && !urls.contains(&url.to_string()) {
                urls.push(url.to_string());
            }
        }
    }
    Ok(urls)
}

pub fn contact_list_to_json(contact_list: &ContactList) -> String {
    let mut json = String::new();
    json.push_str("{\"owner_pubkey\":\"");
    json.push_str(&escape_json_string(&contact_list.owner_pubkey));
    json.push_str("\",\"created_at\":");
    json.push_str(&contact_list.created_at.to_string());
    json.push_str(",\"count\":");
    json.push_str(&contact_list.contacts.len().to_string());
    json.push_str(",\"contacts\":[");
    for (i, contact) in contact_list.contacts.iter().enumerate() {
        json.push_str("{\"pubkey\":\"");
        json.push_str(&escape_json_string(&contact.pubkey));
        json.push_str("\"");
        if let Some(ref relay) = contact.relay_url {
            json.push_str(",\"relay_url\":\"");
            json.push_str(&escape_json_string(relay));
            json.push_str("\"");
        }
        if let Some(ref name) = contact.petname {
            json.push_str(",\"petname\":\"");
            json.push_str(&escape_json_string(name));
            json.push_str("\"");
        }
        json.push_str("}");
        if i < contact_list.contacts.len() - 1 {
            json.push_str(",");
        }
    }
    json.push_str("]}");
    return json;
}

pub struct FollowerInfo {
    pub pubkey: String,
}

/// Compact event JSON (no whitespace) for embedding inside encrypted payloads.
pub fn event_to_json_compact(event: &Event) -> String {
    let mut json = String::new();
    json.push_str("{\"id\":\"");
    json.push_str(&escape_json_string(&event.id));
    json.push_str("\",\"pubkey\":\"");
    json.push_str(&escape_json_string(&event.pubkey));
    json.push_str("\",\"created_at\":");
    json.push_str(&event.created_at.to_string());
    json.push_str(",\"kind\":");
    json.push_str(&event.kind.to_string());
    json.push_str(",\"tags\":[");
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
    json.push_str("],\"content\":\"");
    json.push_str(&escape_json_string(&event.content));
    json.push_str("\",\"sig\":\"");
    json.push_str(&escape_json_string(&event.sig));
    json.push_str("\"}");
    json
}

/// Filter for searching notes by hashtag.
pub fn filter_notes_by_hashtag(hashtag: &str, limit: u32) -> Filter {
    Filter {
        kinds: Some(vec![KIND_TEXT_NOTE, KIND_LONG_FORM]),
        limit: Some(limit),
        t_tags: Some(vec![hashtag.to_lowercase()]),
        ..Default::default()
    }
}

/// Filter for NIP-50 full-text search.
pub fn filter_notes_by_search(query: &str, limit: u32, authors: Option<Vec<String>>, since: Option<u64>, until: Option<u64>) -> Filter {
    let kinds = Some(vec![KIND_TEXT_NOTE, KIND_LONG_FORM]);
    Filter {
        authors,
        kinds,
        since,
        until,
        limit: Some(limit),
        search: Some(query.to_string()),
        ..Default::default()
    }
}

/// Filter for NIP-17 gift wraps addressed to us: kind 1059, #p = our pubkey.
pub fn filter_gift_wraps_received(our_pubkey_hex: &str, limit: u32, since: Option<u64>) -> Filter {
    Filter {
        kinds: Some(vec![KIND_GIFT_WRAP]),
        since,
        limit: Some(limit),
        p_tags: Some(vec![our_pubkey_hex.to_string()]),
        ..Default::default()
    }
}

/// Filter for kind 10050 DM relay list by author.
pub fn filter_dm_relay_list_by_author(author_pubkey: &str) -> Filter {
    Filter {
        authors: Some(vec![author_pubkey.to_string()]),
        kinds: Some(vec![KIND_DM_RELAY_LIST]),
        limit: Some(1),
        ..Default::default()
    }
}

/// Parse kind 10050 DM relay list: extract relay URLs from ["relay", "wss://..."] tags.
pub fn parse_dm_relay_list(event: &Event) -> Result<Vec<String>, String> {
    if event.kind != KIND_DM_RELAY_LIST {
        return Err(format!("Expected kind 10050 event, got kind {}", event.kind));
    }
    let mut urls: Vec<String> = Vec::new();
    for tag in &event.tags {
        if tag.len() >= 2 && tag[0] == "relay" && !tag[1].is_empty() {
            let url = tag[1].trim();
            if !url.is_empty() && !urls.contains(&url.to_string()) {
                urls.push(url.to_string());
            }
        }
    }
    Ok(urls)
}

pub fn followers_to_json(followers: &Vec<FollowerInfo>) -> String {
    let mut json = String::new();
    json.push_str("{\"count\":");
    json.push_str(&followers.len().to_string());
    json.push_str(",\"followers\":[");
    for (i, follower) in followers.iter().enumerate() {
        json.push_str("{\"pubkey\":\"");
        json.push_str(&escape_json_string(&follower.pubkey));
        json.push_str("\"}");
        if i < followers.len() - 1 {
            json.push_str(",");
        }
    }
    json.push_str("]}");
    return json;
}
