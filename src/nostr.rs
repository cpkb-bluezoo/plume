// Plume - Nostr Protocol Handling
// Basic structures and functions for working with Nostr

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
#[allow(dead_code)]
pub const KIND_REPOST: u32 = 6;         // Repost/boost of another note
#[allow(dead_code)]
pub const KIND_REACTION: u32 = 7;       // Reaction (like, emoji)
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
// JSON Parsing Functions
// ============================================================

// Parse a JSON string into a Nostr Event
pub fn parse_event(json_str: &str) -> Result<Event, String> {
    let parsed = match json::parse(json_str) {
        Ok(value) => value,
        Err(e) => return Err(format!("Invalid JSON: {}", e)),
    };
    
    // Extract id (required)
    let id: String;
    if parsed["id"].is_string() {
        id = parsed["id"].as_str().unwrap().to_string();
    } else {
        return Err(String::from("Missing or invalid 'id' field"));
    }
    
    // Extract pubkey (required)
    let pubkey: String;
    if parsed["pubkey"].is_string() {
        pubkey = parsed["pubkey"].as_str().unwrap().to_string();
    } else {
        return Err(String::from("Missing or invalid 'pubkey' field"));
    }
    
    // Extract created_at (required)
    let created_at: u64;
    if parsed["created_at"].is_number() {
        created_at = parsed["created_at"].as_u64().unwrap_or(0);
    } else {
        return Err(String::from("Missing or invalid 'created_at' field"));
    }
    
    // Extract kind (required)
    let kind: u32;
    if parsed["kind"].is_number() {
        kind = parsed["kind"].as_u32().unwrap_or(0);
    } else {
        return Err(String::from("Missing or invalid 'kind' field"));
    }
    
    // Extract content (required)
    let content: String;
    if parsed["content"].is_string() {
        content = parsed["content"].as_str().unwrap().to_string();
    } else {
        content = String::new();
    }
    
    // Extract sig (required)
    let sig: String;
    if parsed["sig"].is_string() {
        sig = parsed["sig"].as_str().unwrap().to_string();
    } else {
        return Err(String::from("Missing or invalid 'sig' field"));
    }
    
    // Extract tags (array of arrays)
    let mut tags: Vec<Vec<String>> = Vec::new();
    if parsed["tags"].is_array() {
        for tag_array in parsed["tags"].members() {
            let mut tag: Vec<String> = Vec::new();
            if tag_array.is_array() {
                for item in tag_array.members() {
                    if item.is_string() {
                        tag.push(item.as_str().unwrap().to_string());
                    }
                }
            }
            tags.push(tag);
        }
    }
    
    let event = Event {
        id: id,
        pubkey: pubkey,
        created_at: created_at,
        kind: kind,
        tags: tags,
        content: content,
        sig: sig,
    };
    
    return Ok(event);
}

// Parse profile metadata from a kind 0 event's content
pub fn parse_profile(content: &str) -> Result<ProfileMetadata, String> {
    let parsed = match json::parse(content) {
        Ok(value) => value,
        Err(e) => return Err(format!("Invalid profile JSON: {}", e)),
    };
    
    let mut profile = ProfileMetadata::new();
    
    // Extract each optional field
    if parsed["name"].is_string() {
        profile.name = Some(parsed["name"].as_str().unwrap().to_string());
    }
    if parsed["about"].is_string() {
        profile.about = Some(parsed["about"].as_str().unwrap().to_string());
    }
    if parsed["picture"].is_string() {
        profile.picture = Some(parsed["picture"].as_str().unwrap().to_string());
    }
    if parsed["nip05"].is_string() {
        profile.nip05 = Some(parsed["nip05"].as_str().unwrap().to_string());
    }
    if parsed["banner"].is_string() {
        profile.banner = Some(parsed["banner"].as_str().unwrap().to_string());
    }
    if parsed["website"].is_string() {
        profile.website = Some(parsed["website"].as_str().unwrap().to_string());
    }
    if parsed["lud16"].is_string() {
        profile.lud16 = Some(parsed["lud16"].as_str().unwrap().to_string());
    }
    
    return Ok(profile);
}

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
        // first = false;  // Not needed, last field
        json.push_str("\"#p\":[");
        for (i, pubkey) in p_tags.iter().enumerate() {
            json.push_str("\"");
            json.push_str(&escape_json_string(pubkey));
            json.push_str("\"");
            if i < p_tags.len() - 1 { json.push_str(","); }
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
    }
}

// Convert a ProfileMetadata to JSON string
pub fn profile_to_json(profile: &ProfileMetadata) -> String {
    let mut json = String::new();
    json.push_str("{");
    
    let mut first = true;
    
    // name
    if let Some(ref name) = profile.name {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"name\":\"");
        json.push_str(&escape_json_string(name));
        json.push_str("\"");
    }
    
    // about
    if let Some(ref about) = profile.about {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"about\":\"");
        json.push_str(&escape_json_string(about));
        json.push_str("\"");
    }
    
    // picture
    if let Some(ref picture) = profile.picture {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"picture\":\"");
        json.push_str(&escape_json_string(picture));
        json.push_str("\"");
    }
    
    // nip05
    if let Some(ref nip05) = profile.nip05 {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"nip05\":\"");
        json.push_str(&escape_json_string(nip05));
        json.push_str("\"");
    }
    
    // banner
    if let Some(ref banner) = profile.banner {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"banner\":\"");
        json.push_str(&escape_json_string(banner));
        json.push_str("\"");
    }
    
    // website
    if let Some(ref website) = profile.website {
        if !first { json.push_str(","); }
        first = false;
        json.push_str("\"website\":\"");
        json.push_str(&escape_json_string(website));
        json.push_str("\"");
    }
    
    // lud16 (lightning address)
    if let Some(ref lud16) = profile.lud16 {
        if !first { json.push_str(","); }
        // first = false;  // Not needed, last field
        json.push_str("\"lud16\":\"");
        json.push_str(&escape_json_string(lud16));
        json.push_str("\"");
    }
    
    if let Some(created_at) = profile.created_at {
        if !first { json.push_str(","); }
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

// A contact entry from a kind 3 event
// Each "p" tag in a kind 3 event represents someone the author follows
pub struct Contact {
    // The public key of the followed user
    pub pubkey: String,
    
    // Optional relay URL where this user can be found
    pub relay_url: Option<String>,
    
    // Optional petname (local nickname)
    pub petname: Option<String>,
}

// A contact list (who a user follows)
pub struct ContactList {
    // The public key of the owner of this contact list
    pub owner_pubkey: String,
    
    // List of contacts (people they follow)
    pub contacts: Vec<Contact>,
    
    // When this contact list was last updated
    pub created_at: u64,
}

// Parse contacts from a kind 3 event
// The "p" tags contain: ["p", pubkey, relay_url?, petname?]
pub fn parse_contact_list(event: &Event) -> Result<ContactList, String> {
    if event.kind != KIND_CONTACTS {
        return Err(format!("Expected kind 3 event, got kind {}", event.kind));
    }
    
    let mut contacts: Vec<Contact> = Vec::new();
    
    // Extract contacts from "p" tags
    for tag in &event.tags {
        if tag.len() >= 2 && tag[0] == "p" {
            let pubkey = tag[1].clone();
            
            // Optional relay URL (index 2)
            let relay_url: Option<String>;
            if tag.len() >= 3 && !tag[2].is_empty() {
                relay_url = Some(tag[2].clone());
            } else {
                relay_url = None;
            }
            
            // Optional petname (index 3)
            let petname: Option<String>;
            if tag.len() >= 4 && !tag[3].is_empty() {
                petname = Some(tag[3].clone());
            } else {
                petname = None;
            }
            
            contacts.push(Contact {
                pubkey: pubkey,
                relay_url: relay_url,
                petname: petname,
            });
        }
    }
    
    return Ok(ContactList {
        owner_pubkey: event.pubkey.clone(),
        contacts: contacts,
        created_at: event.created_at,
    });
}

/// Get just the pubkeys from a contact list.
#[allow(dead_code)]
pub fn get_following_pubkeys(contact_list: &ContactList) -> Vec<String> {
    let mut pubkeys: Vec<String> = Vec::new();
    
    for contact in &contact_list.contacts {
        pubkeys.push(contact.pubkey.clone());
    }
    
    return pubkeys;
}

// Create a filter for a user's contact list (who they follow)
pub fn filter_contact_list_by_author(author_pubkey: &str) -> Filter {
    Filter {
        ids: None,
        authors: Some(vec![author_pubkey.to_string()]),
        kinds: Some(vec![KIND_CONTACTS]),
        since: None,
        until: None,
        limit: Some(1),  // Only need the most recent contact list
        p_tags: None,
    }
}

// Create a filter to find followers (kind 3 events that tag a pubkey)
pub fn filter_followers_by_pubkey(target_pubkey: &str) -> Filter {
    Filter {
        ids: None,
        authors: None,  // Any author
        kinds: Some(vec![KIND_CONTACTS]),
        since: None,
        until: None,
        limit: Some(500),  // Limit to avoid huge responses
        p_tags: Some(vec![target_pubkey.to_string()]),
    }
}

// Create a filter for a user's relay list (NIP-65 kind 10002)
pub fn filter_relay_list_by_author(author_pubkey: &str) -> Filter {
    Filter {
        ids: None,
        authors: Some(vec![author_pubkey.to_string()]),
        kinds: Some(vec![KIND_RELAY_LIST]),
        since: None,
        until: None,
        limit: Some(1),
        p_tags: None,
    }
}

/// Parse relay list from a kind 10002 event (NIP-65). Tags: ["r", "relay_url"] or ["r", "url", "read"/"write"].
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

// Convert a ContactList to JSON string
pub fn contact_list_to_json(contact_list: &ContactList) -> String {
    let mut json = String::new();
    json.push_str("{");
    
    // owner_pubkey
    json.push_str("\"owner_pubkey\":\"");
    json.push_str(&escape_json_string(&contact_list.owner_pubkey));
    json.push_str("\",");
    
    // created_at
    json.push_str("\"created_at\":");
    json.push_str(&contact_list.created_at.to_string());
    json.push_str(",");
    
    // count
    json.push_str("\"count\":");
    json.push_str(&contact_list.contacts.len().to_string());
    json.push_str(",");
    
    // contacts array
    json.push_str("\"contacts\":[");
    for (i, contact) in contact_list.contacts.iter().enumerate() {
        json.push_str("{");
        
        // pubkey
        json.push_str("\"pubkey\":\"");
        json.push_str(&escape_json_string(&contact.pubkey));
        json.push_str("\"");
        
        // relay_url (optional)
        if let Some(ref relay) = contact.relay_url {
            json.push_str(",\"relay_url\":\"");
            json.push_str(&escape_json_string(relay));
            json.push_str("\"");
        }
        
        // petname (optional)
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
    json.push_str("]");
    
    json.push_str("}");
    return json;
}

// Simple struct for follower info
pub struct FollowerInfo {
    pub pubkey: String,
}

// Convert followers list to JSON
pub fn followers_to_json(followers: &Vec<FollowerInfo>) -> String {
    let mut json = String::new();
    json.push_str("{");
    
    // count
    json.push_str("\"count\":");
    json.push_str(&followers.len().to_string());
    json.push_str(",");
    
    // followers array
    json.push_str("\"followers\":[");
    for (i, follower) in followers.iter().enumerate() {
        json.push_str("{\"pubkey\":\"");
        json.push_str(&escape_json_string(&follower.pubkey));
        json.push_str("\"}");
        
        if i < followers.len() - 1 {
            json.push_str(",");
        }
    }
    json.push_str("]");
    
    json.push_str("}");
    return json;
}
