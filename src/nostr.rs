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
pub const KIND_ZAP_REQUEST: u32 = 9734; // NIP-57 Lightning zap request
#[allow(dead_code)]
pub const KIND_LONG_FORM: u32 = 30023;  // Long-form content (articles)
/// NIP-65: Relay list metadata (tags: ["r", "relay_url"] or ["r", "url", "read"/"write"])
pub const KIND_RELAY_LIST: u32 = 10002;

// A filter for requesting events from relays
#[derive(Clone)]
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
            sig: self.sig.clone().ok_or("Missing 'sig' field")?,
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
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"ids\":[");
        for (i, id) in ids.iter().enumerate() {
            json.push_str("\"");
            json.push_str(&escape_json_string(id));
            json.push_str("\"");
            if i < ids.len() - 1 { json.push_str(","); }
        }
        json.push_str("]");
    }
    
    // authors
    if let Some(ref authors) = filter.authors {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"authors\":[");
        for (i, author) in authors.iter().enumerate() {
            json.push_str("\"");
            json.push_str(&escape_json_string(author));
            json.push_str("\"");
            if i < authors.len() - 1 { json.push_str(","); }
        }
        json.push_str("]");
    }
    
    // kinds
    if let Some(ref kinds) = filter.kinds {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"kinds\":[");
        for (i, kind) in kinds.iter().enumerate() {
            json.push_str(&kind.to_string());
            if i < kinds.len() - 1 { json.push_str(","); }
        }
        json.push_str("]");
    }
    
    // since
    if let Some(since) = filter.since {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"since\":");
        json.push_str(&since.to_string());
    }
    
    // until
    if let Some(until) = filter.until {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"until\":");
        json.push_str(&until.to_string());
    }
    
    // limit
    if let Some(limit) = filter.limit {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"limit\":");
        json.push_str(&limit.to_string());
    }
    
    // #p tags (for filtering by referenced pubkeys)
    if let Some(ref p_tags) = filter.p_tags {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"#p\":[");
        for (i, pubkey) in p_tags.iter().enumerate() {
            json.push_str("\"");
            json.push_str(&escape_json_string(pubkey));
            json.push_str("\"");
            if i < p_tags.len() - 1 { json.push_str(","); }
        }
        json.push_str("]");
    }

    // #e tags (for filtering by referenced event IDs, e.g. replies)
    if let Some(ref e_tags) = filter.e_tags {
        if !first { json.push_str(","); }
        let _ = first;
        json.push_str("\"#e\":[");
        for (i, eid) in e_tags.iter().enumerate() {
            json.push_str("\"");
            json.push_str(&escape_json_string(eid));
            json.push_str("\"");
            if i < e_tags.len() - 1 { json.push_str(","); }
        }
        json.push_str("]");
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
        ids: None,
        authors: Some(authors),
        kinds: Some(vec![KIND_TEXT_NOTE]),
        since,
        until: None,
        limit: Some(limit),
        p_tags: None,
        e_tags: None,
    }
}

/// Profile feed: notes (kind 1) and reposts (kind 6) by authors. Used so reposts appear on profile.
pub fn filter_profile_feed_by_authors_since(authors: Vec<String>, limit: u32, since: Option<u64>) -> Filter {
    Filter {
        ids: None,
        authors: Some(authors),
        kinds: Some(vec![KIND_TEXT_NOTE, KIND_REPOST]),
        since,
        until: None,
        limit: Some(limit),
        p_tags: None,
        e_tags: None,
    }
}

// Create a filter for recent global notes (optionally only since timestamp)
pub fn filter_recent_notes(limit: u32) -> Filter {
    filter_recent_notes_since(limit, None)
}

pub fn filter_recent_notes_since(limit: u32, since: Option<u64>) -> Filter {
    Filter {
        ids: None,
        authors: None,
        kinds: Some(vec![KIND_TEXT_NOTE]),
        since,
        until: None,
        limit: Some(limit),
        p_tags: None,
        e_tags: None,
    }
}

/// Create a filter to fetch kind 1 notes that reference the given event ID in an "e" tag (replies).
pub fn filter_replies_to_event(event_id: String, limit: u32) -> Filter {
    Filter {
        ids: None,
        authors: None,
        kinds: Some(vec![KIND_TEXT_NOTE]),
        since: None,
        until: None,
        limit: Some(limit),
        p_tags: None,
        e_tags: Some(vec![event_id]),
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
        ids: None,
        authors: None,
        kinds: Some(vec![KIND_DM]),
        since,
        until: None,
        limit: Some(limit),
        p_tags: Some(vec![our_pubkey_hex.to_string()]),
        e_tags: None,
    }
}

/// Filter for DMs we sent: kind 4 with authors = our pubkey.
pub fn filter_dms_sent(our_pubkey_hex: &str, limit: u32, since: Option<u64>) -> Filter {
    Filter {
        ids: None,
        authors: Some(vec![our_pubkey_hex.to_string()]),
        kinds: Some(vec![KIND_DM]),
        since,
        until: None,
        limit: Some(limit),
        p_tags: None,
        e_tags: None,
    }
}

/// Create a filter to fetch events by their IDs (e.g. for bookmarks).
pub fn filter_events_by_ids(ids: Vec<String>) -> Filter {
    Filter {
        ids: Some(ids),
        authors: None,
        kinds: Some(vec![KIND_TEXT_NOTE]),
        since: None,
        until: None,
        limit: None,
        p_tags: None,
        e_tags: None,
    }
}

// Create a filter for profile metadata (kind 0) by author
pub fn filter_profile_by_author(author_pubkey: &str) -> Filter {
    Filter {
        ids: None,
        authors: Some(vec![author_pubkey.to_string()]),
        kinds: Some(vec![KIND_METADATA]),
        since: None,
        until: None,
        limit: Some(1),  // Only need the most recent profile
        p_tags: None,
        e_tags: None,
    }
}

/// Create a filter for multiple profiles at once.
#[allow(dead_code)]
pub fn filter_profiles_by_authors(author_pubkeys: Vec<String>) -> Filter {
    Filter {
        ids: None,
        authors: Some(author_pubkeys),
        kinds: Some(vec![KIND_METADATA]),
        since: None,
        until: None,
        limit: None,  // Get all matching profiles
        p_tags: None,
        e_tags: None,
    }
}

// Convert a ProfileMetadata to JSON string
pub fn profile_to_json(profile: &ProfileMetadata) -> String {
    let mut json = String::new();
    json.push_str("{");
    
    let mut first = true;
    
    if let Some(ref name) = profile.name {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"name\":\"");
        json.push_str(&escape_json_string(name));
        json.push_str("\"");
    }
    if let Some(ref about) = profile.about {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"about\":\"");
        json.push_str(&escape_json_string(about));
        json.push_str("\"");
    }
    if let Some(ref picture) = profile.picture {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"picture\":\"");
        json.push_str(&escape_json_string(picture));
        json.push_str("\"");
    }
    if let Some(ref nip05) = profile.nip05 {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"nip05\":\"");
        json.push_str(&escape_json_string(nip05));
        json.push_str("\"");
    }
    if let Some(ref banner) = profile.banner {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"banner\":\"");
        json.push_str(&escape_json_string(banner));
        json.push_str("\"");
    }
    if let Some(ref website) = profile.website {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"website\":\"");
        json.push_str(&escape_json_string(website));
        json.push_str("\"");
    }
    if let Some(ref lud16) = profile.lud16 {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"lud16\":\"");
        json.push_str(&escape_json_string(lud16));
        json.push_str("\"");
    }
    if let Some(created_at) = profile.created_at {
        if !first { json.push_str(","); }
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
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"name\":\"");
        json.push_str(&escape_json_string(name));
        json.push_str("\"");
    }
    if let Some(ref about) = profile.about {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"about\":\"");
        json.push_str(&escape_json_string(about));
        json.push_str("\"");
    }
    if let Some(ref picture) = profile.picture {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"picture\":\"");
        json.push_str(&escape_json_string(picture));
        json.push_str("\"");
    }
    if let Some(ref nip05) = profile.nip05 {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"nip05\":\"");
        json.push_str(&escape_json_string(nip05));
        json.push_str("\"");
    }
    if let Some(ref banner) = profile.banner {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"banner\":\"");
        json.push_str(&escape_json_string(banner));
        json.push_str("\"");
    }
    if let Some(ref website) = profile.website {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"website\":\"");
        json.push_str(&escape_json_string(website));
        json.push_str("\"");
    }
    if let Some(ref lud16) = profile.lud16 {
        if !first { json.push_str(","); }
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
        ids: None,
        authors: Some(vec![author_pubkey.to_string()]),
        kinds: Some(vec![KIND_CONTACTS]),
        since: None,
        until: None,
        limit: Some(1),
        p_tags: None,
        e_tags: None,
    }
}

pub fn filter_followers_by_pubkey(target_pubkey: &str) -> Filter {
    Filter {
        ids: None,
        authors: None,
        kinds: Some(vec![KIND_CONTACTS]),
        since: None,
        until: None,
        limit: Some(500),
        p_tags: Some(vec![target_pubkey.to_string()]),
        e_tags: None,
    }
}

pub fn filter_relay_list_by_author(author_pubkey: &str) -> Filter {
    Filter {
        ids: None,
        authors: Some(vec![author_pubkey.to_string()]),
        kinds: Some(vec![KIND_RELAY_LIST]),
        since: None,
        until: None,
        limit: Some(1),
        p_tags: None,
        e_tags: None,
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
