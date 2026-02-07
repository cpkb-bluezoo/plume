// Plume - A Nostr Desktop Client
// Main entry point

// Disable the default Windows console window in release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Import our modules
mod config;
mod crypto;
mod keys;
mod nostr;
mod relay;

// Import what we need from external crates
use tauri::{Emitter, Manager};

// Application state that persists while the app is running
struct AppState {
    config_dir: String,
}

// ============================================================
// Configuration Commands
// ============================================================

// Tauri command: Get the configuration directory path
// Commands are functions that the frontend JavaScript can call
#[tauri::command]
fn get_config_dir(state: tauri::State<AppState>) -> String {
    return state.config_dir.clone();
}

// Tauri command: Load the user's configuration
#[tauri::command]
fn load_config(state: tauri::State<AppState>) -> Result<String, String> {
    let config_dir = &state.config_dir;
    
    match config::load_config(config_dir) {
        Ok(cfg) => {
            // Convert config to JSON string for the frontend
            let json = config::config_to_json(&cfg);
            return Ok(json);
        }
        Err(e) => {
            return Err(format!("Failed to load config: {}", e));
        }
    }
}

// Tauri command: Save the user's configuration
#[tauri::command]
fn save_config(state: tauri::State<AppState>, config_json: String) -> Result<(), String> {
    let config_dir = &state.config_dir;
    
    // Parse the JSON string into a Config struct
    let mut cfg = match config::json_to_config(&config_json) {
        Ok(c) => c,
        Err(e) => return Err(format!("Invalid config JSON: {}", e)),
    };
    // Preserve profile fields from disk if the frontend didn't send them (e.g. old state.config)
    if let Ok(existing) = config::load_config(config_dir) {
        if cfg.profile_picture.is_none() && existing.profile_picture.is_some() {
            cfg.profile_picture = existing.profile_picture.clone();
        }
        if cfg.profile_metadata.is_none() && existing.profile_metadata.is_some() {
            cfg.profile_metadata = existing.profile_metadata.clone();
        }
    }
    
    // Save it to disk
    match config::save_config(config_dir, &cfg) {
        Ok(()) => return Ok(()),
        Err(e) => return Err(format!("Failed to save config: {}", e)),
    }
}

// ============================================================
// Key Conversion Commands
// ============================================================

// Tauri command: Convert a public key to hex format
// Accepts npub or hex, returns hex
#[tauri::command]
fn convert_public_key_to_hex(key: String) -> Result<String, String> {
    return keys::public_key_to_hex(&key);
}

// Tauri command: Convert a hex public key to npub format
#[tauri::command]
fn convert_hex_to_npub(hex_key: String) -> Result<String, String> {
    return keys::hex_to_npub(&hex_key);
}

// Tauri command: Convert a secret key to hex format
// Accepts nsec or hex, returns hex
#[tauri::command]
fn convert_secret_key_to_hex(key: String) -> Result<String, String> {
    return keys::secret_key_to_hex(&key);
}

// Tauri command: Convert a hex secret key to nsec format
#[tauri::command]
fn convert_hex_to_nsec(hex_key: String) -> Result<String, String> {
    return keys::hex_to_nsec(&hex_key);
}

// Tauri command: Validate and get info about a key
// Returns JSON with key type, hex value, and bech32 value
#[tauri::command]
fn parse_key(key: String) -> Result<String, String> {
    let trimmed = key.trim();
    
    // Try as public key (npub or hex)
    if keys::is_npub(trimmed) {
        match keys::npub_to_hex(trimmed) {
            Ok(hex) => {
                let json = format!(
                    "{{\"type\":\"public\",\"hex\":\"{}\",\"npub\":\"{}\"}}",
                    hex, trimmed
                );
                return Ok(json);
            }
            Err(e) => return Err(format!("Invalid npub: {}", e)),
        }
    }
    
    // Try as secret key (nsec)
    if keys::is_nsec(trimmed) {
        match keys::nsec_to_hex(trimmed) {
            Ok(hex) => {
                let json = format!(
                    "{{\"type\":\"secret\",\"hex\":\"{}\",\"nsec\":\"{}\"}}",
                    hex, trimmed
                );
                return Ok(json);
            }
            Err(e) => return Err(format!("Invalid nsec: {}", e)),
        }
    }
    
    // Try as hex key (could be public or secret - we don't know)
    if keys::is_valid_hex_key(trimmed) {
        let hex = trimmed.to_lowercase();
        let npub = keys::hex_to_npub(&hex).unwrap_or_default();
        let nsec = keys::hex_to_nsec(&hex).unwrap_or_default();
        
        let json = format!(
            "{{\"type\":\"hex\",\"hex\":\"{}\",\"npub\":\"{}\",\"nsec\":\"{}\"}}",
            hex, npub, nsec
        );
        return Ok(json);
    }
    
    return Err(String::from("Invalid key format. Expected npub1..., nsec1..., or 64-char hex"));
}

// ============================================================
// Relay Commands
// ============================================================

// Tauri command: Fetch recent notes from a relay
#[tauri::command]
fn fetch_notes(relay_url: String, limit: u32) -> Result<String, String> {
    println!("Fetching {} notes from {}", limit, relay_url);
    
    // Create a filter for recent text notes
    let filter = nostr::filter_recent_notes(limit);
    
    // Fetch from the relay (with 10 second timeout)
    let events = relay::fetch_notes_from_relay(&relay_url, &filter, 10)?;
    
    // Convert events to JSON array for the frontend
    let json = events_to_json_array(&events);
    
    return Ok(json);
}

// Tauri command: Fetch notes from multiple relays.
// authors: if Some and non-empty, only notes from these pubkeys (hex); else firehose.
// since: if Some, only notes with created_at >= since (for incremental poll).
#[tauri::command]
fn fetch_notes_from_relays(
    relay_urls: Vec<String>,
    limit: u32,
    authors: Option<Vec<String>>,
    since: Option<u64>,
) -> Result<String, String> {
    let use_follows = authors.as_ref().map(|a| !a.is_empty()).unwrap_or(false);
    println!(
        "Fetching notes from {} relays (follows={}, since={:?})",
        relay_urls.len(),
        use_follows,
        since
    );

    let filter = if use_follows {
        nostr::filter_notes_by_authors_since(authors.unwrap(), limit, since)
    } else {
        nostr::filter_recent_notes_since(limit, since)
    };

    let mut all_events: Vec<nostr::Event> = Vec::new();

    for relay_url in relay_urls {
        match relay::fetch_notes_from_relay(&relay_url, &filter, 10) {
            Ok(events) => {
                for event in events {
                    all_events.push(event);
                }
            }
            Err(e) => {
                println!("Error fetching from {}: {}", relay_url, e);
            }
        }
    }

    // Sort by created_at (newest first)
    all_events.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    // Remove duplicates by event ID
    let mut seen_ids: Vec<String> = Vec::new();
    let mut unique_events: Vec<nostr::Event> = Vec::new();
    for event in all_events {
        if !seen_ids.contains(&event.id) {
            seen_ids.push(event.id.clone());
            unique_events.push(event);
        }
    }

    if unique_events.len() > limit as usize {
        unique_events.truncate(limit as usize);
    }

    let json = events_to_json_array(&unique_events);
    println!("Returning {} unique events", unique_events.len());
    Ok(json)
}

// Tauri command: Start streaming feed; emits "feed-note" per event and "feed-eose" when done.
// If stream_context == "profile", emits "profile-feed-note" and "profile-feed-eose" instead (so profile page doesn't conflict with home).
// Uses tokio-tungstenite (selector-based I/O) and per-relay Actson push-parser pipelines; events are sent as soon as they are parsed.
#[tauri::command]
fn start_feed_stream(
    app: tauri::AppHandle,
    relay_urls: Vec<String>,
    limit: u32,
    authors: Option<Vec<String>>,
    since: Option<u64>,
    stream_context: Option<String>,
) -> Result<(), String> {
    let use_follows = authors.as_ref().map(|a| !a.is_empty()).unwrap_or(false);
    let is_profile = stream_context.as_deref() == Some("profile");
    println!(
        "Starting feed stream from {} relays (follows={}, since={:?}, profile={})",
        relay_urls.len(),
        use_follows,
        since,
        is_profile
    );

    let filter = if use_follows {
        nostr::filter_notes_by_authors_since(
            authors.unwrap_or_default(),
            limit,
            since,
        )
    } else {
        nostr::filter_recent_notes_since(limit, since)
    };

    let (note_event, eose_event) = if is_profile {
        ("profile-feed-note".to_string(), "profile-feed-eose".to_string())
    } else {
        ("feed-note".to_string(), "feed-eose".to_string())
    };

    let num_relays = relay_urls.len() as u32;
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(r) => r,
            Err(e) => {
                println!("Failed to create Tokio runtime: {}", e);
                return;
            }
        };
        rt.block_on(async move {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            for relay_url in relay_urls {
                let filter = filter.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    relay::run_relay_feed_stream(
                        relay_url,
                        filter,
                        10,
                        tx,
                    )
                    .await;
                });
            }
            drop(tx);

            let mut eose_count = 0u32;
            while let Some(msg) = rx.recv().await {
                match msg {
                    relay::StreamMessage::Event(event) => {
                        let json = nostr::event_to_json(&event);
                        let _ = app.emit(&note_event, &json);
                    }
                    relay::StreamMessage::Eose => {
                        eose_count += 1;
                        if eose_count >= num_relays {
                            let _ = app.emit(&eose_event, ());
                            break;
                        }
                    }
                    relay::StreamMessage::Notice(msg) => {
                        println!("Relay notice: {}", msg);
                    }
                }
            }
        });
    });

    Ok(())
}

// Tauri command: Test connection to a relay
#[tauri::command]
fn test_relay_connection(relay_url: String) -> Result<String, String> {
    println!("Testing connection to: {}", relay_url);
    
    let mut connection = relay::RelayConnection::new(&relay_url);
    
    match connection.connect() {
        Ok(()) => {
            connection.disconnect();
            return Ok(String::from("Connection successful"));
        }
        Err(e) => {
            return Err(e);
        }
    }
}

// ============================================================
// Profile Commands
// ============================================================

// Tauri command: Fetch profile metadata for a public key
#[tauri::command]
fn fetch_profile(pubkey: String, relay_urls: Vec<String>) -> Result<String, String> {
    println!("Fetching profile for: {}", pubkey);
    
    // Convert key to hex if it's in npub format
    let hex_pubkey = match keys::public_key_to_hex(&pubkey) {
        Ok(hex) => hex,
        Err(e) => return Err(format!("Invalid public key: {}", e)),
    };
    
    // Fetch from relays (5 second timeout per relay)
    match relay::fetch_profile_from_relays(&relay_urls, &hex_pubkey, 5) {
        Ok(Some(profile)) => {
            let json = nostr::profile_to_json(&profile);
            return Ok(json);
        }
        Ok(None) => {
            // No profile found - return empty object
            return Ok(String::from("{}"));
        }
        Err(e) => {
            return Err(format!("Failed to fetch profile: {}", e));
        }
    }
}

// Tauri command: Fetch own profile (using config's public key)
#[tauri::command]
fn fetch_own_profile(state: tauri::State<AppState>) -> Result<String, String> {
    // Load config to get public key and relays
    let config_dir = &state.config_dir;
    
    let cfg = match config::load_config(config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    
    // Check if public key is configured
    if cfg.public_key.is_empty() {
        return Err(String::from("No public key configured"));
    }
    
    // Fetch profile from configured relays
    match relay::fetch_profile_from_relays(&cfg.relays, &cfg.public_key, 5) {
        Ok(Some(profile)) => {
            let json = nostr::profile_to_json(&profile);
            return Ok(json);
        }
        Ok(None) => {
            // No profile found - return empty object
            return Ok(String::from("{}"));
        }
        Err(e) => {
            return Err(format!("Failed to fetch profile: {}", e));
        }
    }
}

// ============================================================
// Verification Commands
// ============================================================

// Tauri command: Verify an event's signature
#[tauri::command]
fn verify_event(event_json: String) -> Result<String, String> {
    // Parse the event
    let event = match nostr::parse_event(&event_json) {
        Ok(e) => e,
        Err(e) => return Err(format!("Invalid event JSON: {}", e)),
    };
    
    // Verify the event
    let result = crypto::verify_event(&event)?;
    
    // Return the result as JSON
    return Ok(crypto::verification_result_to_json(&result));
}

// Tauri command: Verify just the event ID
#[tauri::command]
fn verify_event_id(event_json: String) -> Result<bool, String> {
    // Parse the event
    let event = match nostr::parse_event(&event_json) {
        Ok(e) => e,
        Err(e) => return Err(format!("Invalid event JSON: {}", e)),
    };
    
    // Verify the ID
    return crypto::verify_event_id(&event);
}

// Tauri command: Verify just the signature
#[tauri::command]
fn verify_event_signature(event_json: String) -> Result<bool, String> {
    // Parse the event
    let event = match nostr::parse_event(&event_json) {
        Ok(e) => e,
        Err(e) => return Err(format!("Invalid event JSON: {}", e)),
    };
    
    // Verify the signature
    return crypto::verify_event_signature(&event);
}

// Tauri command: Compute event ID from event data
#[tauri::command]
fn compute_event_id(event_json: String) -> Result<String, String> {
    // Parse the event
    let event = match nostr::parse_event(&event_json) {
        Ok(e) => e,
        Err(e) => return Err(format!("Invalid event JSON: {}", e)),
    };
    
    // Compute and return the ID
    return crypto::compute_event_id(&event);
}

// ============================================================
// Following / Followers Commands
// ============================================================

// Tauri command: Fetch who a user follows (their contact list)
#[tauri::command]
fn fetch_following(pubkey: String, relay_urls: Vec<String>) -> Result<String, String> {
    println!("Fetching following for: {}", pubkey);
    
    // Convert key to hex if it's in npub format
    let hex_pubkey = match keys::public_key_to_hex(&pubkey) {
        Ok(hex) => hex,
        Err(e) => return Err(format!("Invalid public key: {}", e)),
    };
    
    // Fetch from relays
    match relay::fetch_following_from_relays(&relay_urls, &hex_pubkey, 10) {
        Ok(Some(contact_list)) => {
            let json = nostr::contact_list_to_json(&contact_list);
            return Ok(json);
        }
        Ok(None) => {
            // No contact list found - return empty
            return Ok(String::from("{\"owner_pubkey\":\"\",\"created_at\":0,\"count\":0,\"contacts\":[]}"));
        }
        Err(e) => {
            return Err(format!("Failed to fetch following: {}", e));
        }
    }
}

// Tauri command: Fetch own following (using config's public key)
#[tauri::command]
fn fetch_own_following(state: tauri::State<AppState>) -> Result<String, String> {
    // Load config
    let config_dir = &state.config_dir;
    let cfg = match config::load_config(config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    
    if cfg.public_key.is_empty() {
        return Err(String::from("No public key configured"));
    }
    
    // Fetch from configured relays
    match relay::fetch_following_from_relays(&cfg.relays, &cfg.public_key, 10) {
        Ok(Some(contact_list)) => {
            let json = nostr::contact_list_to_json(&contact_list);
            return Ok(json);
        }
        Ok(None) => {
            return Ok(String::from("{\"owner_pubkey\":\"\",\"created_at\":0,\"count\":0,\"contacts\":[]}"));
        }
        Err(e) => {
            return Err(format!("Failed to fetch following: {}", e));
        }
    }
}

// Tauri command: Fetch who follows a user
#[tauri::command]
fn fetch_followers(pubkey: String, relay_urls: Vec<String>) -> Result<String, String> {
    println!("Fetching followers for: {}", pubkey);
    
    // Convert key to hex if it's in npub format
    let hex_pubkey = match keys::public_key_to_hex(&pubkey) {
        Ok(hex) => hex,
        Err(e) => return Err(format!("Invalid public key: {}", e)),
    };
    
    // Fetch from relays
    match relay::fetch_followers_from_relays(&relay_urls, &hex_pubkey, 10) {
        Ok(followers) => {
            let json = nostr::followers_to_json(&followers);
            return Ok(json);
        }
        Err(e) => {
            return Err(format!("Failed to fetch followers: {}", e));
        }
    }
}

// Tauri command: Fetch own followers (using config's public key)
#[tauri::command]
fn fetch_own_followers(state: tauri::State<AppState>) -> Result<String, String> {
    // Load config
    let config_dir = &state.config_dir;
    let cfg = match config::load_config(config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    
    if cfg.public_key.is_empty() {
        return Err(String::from("No public key configured"));
    }
    
    // Fetch from configured relays
    match relay::fetch_followers_from_relays(&cfg.relays, &cfg.public_key, 10) {
        Ok(followers) => {
            let json = nostr::followers_to_json(&followers);
            return Ok(json);
        }
        Err(e) => {
            return Err(format!("Failed to fetch followers: {}", e));
        }
    }
}

// Tauri command: Fetch a user's relay list (NIP-65 kind 10002). Returns JSON array of relay URLs.
#[tauri::command]
fn fetch_relay_list(pubkey: String, relay_urls: Vec<String>) -> Result<String, String> {
    let hex_pubkey = match keys::public_key_to_hex(&pubkey) {
        Ok(hex) => hex,
        Err(e) => return Err(format!("Invalid public key: {}", e)),
    };
    match relay::fetch_relay_list_from_relays(&relay_urls, &hex_pubkey, 10) {
        Ok(urls) => {
            let mut json = String::from("[");
            for (i, url) in urls.iter().enumerate() {
                if i > 0 {
                    json.push_str(",");
                }
                json.push('"');
                json.push_str(&url.replace('\\', "\\\\").replace('"', "\\\""));
                json.push('"');
            }
            json.push(']');
            Ok(json)
        }
        Err(e) => Err(format!("Failed to fetch relay list: {}", e)),
    }
}

// ============================================================
// Posting / Signing Commands
// ============================================================

// Tauri command: Post a new text note (optionally a reply: reply_to_event_id + reply_to_pubkey).
#[tauri::command]
fn post_note(
    state: tauri::State<AppState>,
    content: String,
    reply_to_event_id: Option<String>,
    reply_to_pubkey: Option<String>,
) -> Result<String, String> {
    println!("Posting note: {}", &content[..std::cmp::min(50, content.len())]);

    let config_dir = &state.config_dir;
    let cfg = match config::load_config(config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };

    let secret_key = match cfg.private_key {
        Some(key) => key,
        None => return Err(String::from("No private key configured. Add your nsec in Settings to post notes.")),
    };

    if cfg.relays.is_empty() {
        return Err(String::from("No relays configured"));
    }

    // Build tags for reply (NIP-10: e and p tags)
    let mut tags: Vec<Vec<String>> = Vec::new();
    if let (Some(eid), Some(pk)) = (reply_to_event_id, reply_to_pubkey) {
        if !eid.is_empty() && !pk.is_empty() {
            tags.push(vec![String::from("e"), eid, String::new(), String::from("reply")]);
            tags.push(vec![String::from("p"), pk]);
        }
    }

    let event = match crypto::create_signed_note(&content, &secret_key, tags) {
        Ok(e) => e,
        Err(e) => return Err(format!("Failed to create note: {}", e)),
    };
    
    // Publish to relays
    let results = relay::publish_event_to_relays(&cfg.relays, &event, 10);
    
    // Check if at least one relay accepted it
    let success_count = results.iter().filter(|r| r.success).count();
    
    if success_count == 0 {
        return Err(String::from("Failed to publish to any relay"));
    }
    
    // Return the results
    let json = relay::publish_results_to_json(&results);
    return Ok(json);
}

// Tauri command: Set profile metadata (publish kind 0 event). profile_json: JSON with optional name, about, picture, nip05, banner, website, lud16.
// Updates local config (.plume) with the new display name and publishes the profile to the user's relays.
#[tauri::command]
fn set_profile_metadata(state: tauri::State<AppState>, profile_json: String) -> Result<String, String> {
    let config_dir = &state.config_dir;
    let mut cfg = match config::load_config(config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    let secret_key = match cfg.private_key.as_ref() {
        Some(k) => k.as_str(),
        None => return Err(String::from("No private key configured. Add your nsec in Settings to update profile.")),
    };
    if cfg.relays.is_empty() {
        return Err(String::from("No relays configured"));
    }
    let profile = match nostr::parse_profile(&profile_json) {
        Ok(p) => p,
        Err(e) => return Err(format!("Invalid profile JSON: {}", e)),
    };
    let content = nostr::profile_to_content(&profile);
    let event = match crypto::create_signed_metadata_event(&content, secret_key) {
        Ok(e) => e,
        Err(e) => return Err(format!("Failed to create profile event: {}", e)),
    };
    let results = relay::publish_event_to_relays(&cfg.relays, &event, 10);
    let success_count = results.iter().filter(|r| r.success).count();
    if success_count == 0 {
        return Err(String::from("Failed to publish profile to any relay"));
    }
    // Save full profile to local config (.plume)
    if let Some(ref name) = profile.name {
        cfg.display_name = name.clone();
    }
    cfg.profile_picture = profile.picture.clone();
    cfg.profile_metadata = Some(content);
    if let Err(e) = config::save_config(config_dir, &cfg) {
        return Err(format!("Profile published but failed to save local config: {}", e));
    }
    let json = relay::publish_results_to_json(&results);
    Ok(json)
}

// Tauri command: Sign an event (without publishing)
#[tauri::command]
fn sign_event(state: tauri::State<AppState>, event_json: String) -> Result<String, String> {
    // Load config
    let config_dir = &state.config_dir;
    let cfg = match config::load_config(config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    
    // Check for private key
    let secret_key = match cfg.private_key {
        Some(key) => key,
        None => return Err(String::from("No private key configured")),
    };
    
    // Parse the event
    let mut event = match nostr::parse_event(&event_json) {
        Ok(e) => e,
        Err(e) => return Err(format!("Invalid event JSON: {}", e)),
    };
    
    // Sign it
    match crypto::sign_event(&mut event, &secret_key) {
        Ok(()) => {}
        Err(e) => return Err(format!("Failed to sign event: {}", e)),
    };
    
    // Return the signed event
    return Ok(nostr::event_to_json(&event));
}

// Tauri command: Get public key from configured private key
#[tauri::command]
fn get_derived_public_key(state: tauri::State<AppState>) -> Result<String, String> {
    // Load config
    let config_dir = &state.config_dir;
    let cfg = match config::load_config(config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    
    // Check for private key
    let secret_key = match cfg.private_key {
        Some(key) => key,
        None => return Err(String::from("No private key configured")),
    };
    
    // Derive public key
    let pubkey = crypto::get_public_key_from_secret(&secret_key)?;
    
    // Also get npub format
    let npub = match keys::hex_to_npub(&pubkey) {
        Ok(n) => n,
        Err(_) => String::new(),
    };
    
    // Return as JSON
    let json = format!("{{\"hex\":\"{}\",\"npub\":\"{}\"}}", pubkey, npub);
    return Ok(json);
}

// Tauri command: Generate a new key pair
#[tauri::command]
fn generate_keypair(state: tauri::State<AppState>) -> Result<String, String> {
    // Generate the key pair
    let (secret_hex, pubkey_hex) = crypto::generate_keypair()?;
    
    // Convert to bech32 formats
    let npub = match keys::hex_to_npub(&pubkey_hex) {
        Ok(n) => n,
        Err(_) => String::new(),
    };
    
    let nsec = match keys::hex_to_nsec(&secret_hex) {
        Ok(n) => n,
        Err(_) => String::new(),
    };
    
    // Save to config
    let config_dir = &state.config_dir;
    let mut cfg = match config::load_config(config_dir) {
        Ok(c) => c,
        Err(_) => config::Config {
            display_name: String::new(),
            public_key: String::new(),
            private_key: None,
            relays: vec![
                String::from("wss://relay.damus.io"),
                String::from("wss://nos.lol"),
                String::from("wss://relay.nostr.band"),
            ],
            profile_picture: None,
            profile_metadata: None,
            home_feed_mode: String::from("firehose"),
        },
    };
    
    cfg.public_key = pubkey_hex.clone();
    cfg.private_key = Some(secret_hex.clone());
    
    match config::save_config(config_dir, &cfg) {
        Ok(()) => {}
        Err(e) => return Err(format!("Failed to save config: {}", e)),
    }
    
    // Return the keys as JSON
    let json = format!(
        "{{\"public_key_hex\":\"{}\",\"private_key_hex\":\"{}\",\"npub\":\"{}\",\"nsec\":\"{}\"}}",
        pubkey_hex, secret_hex, npub, nsec
    );
    return Ok(json);
}

// ============================================================
// Helper Functions
// ============================================================

// Helper: Convert a vector of events to a JSON array string
fn events_to_json_array(events: &Vec<nostr::Event>) -> String {
    let mut json = String::from("[");
    
    for (index, event) in events.iter().enumerate() {
        json.push_str(&nostr::event_to_json(event));
        
        if index < events.len() - 1 {
            json.push_str(",");
        }
    }
    
    json.push_str("]");
    return json;
}

// ============================================================
// Main Entry Point
// ============================================================

fn main() {
    // Figure out where the config directory should be
    // This will be $HOME/.plume on Unix systems
    let config_dir: String = match config::get_config_dir() {
        Some(path) => path,
        None => {
            eprintln!("ERROR: Could not determine home directory");
            std::process::exit(1);
        }
    };
    
    // Make sure the config directory exists
    match config::ensure_config_dir(&config_dir) {
        Ok(()) => {
            println!("Config directory ready: {}", config_dir);
        }
        Err(e) => {
            eprintln!("ERROR: Could not create config directory: {}", e);
            std::process::exit(1);
        }
    }
    
    // Create the application state
    let app_state = AppState {
        config_dir: config_dir,
    };
    
    // Build and run the Tauri application
    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            // Config commands
            get_config_dir,
            load_config,
            save_config,
            // Key commands
            convert_public_key_to_hex,
            convert_hex_to_npub,
            convert_secret_key_to_hex,
            convert_hex_to_nsec,
            parse_key,
            // Relay commands
            fetch_notes,
            fetch_notes_from_relays,
            start_feed_stream,
            test_relay_connection,
            // Profile commands
            fetch_profile,
            fetch_own_profile,
            set_profile_metadata,
            // Verification commands
            verify_event,
            verify_event_id,
            verify_event_signature,
            compute_event_id,
            // Following / Followers commands
            fetch_following,
            fetch_own_following,
            fetch_followers,
            fetch_own_followers,
            fetch_relay_list,
            // Posting / Signing commands
            post_note,
            sign_event,
            get_derived_public_key,
            generate_keypair,
        ])
        .setup(|app| {
            // This runs once when the app starts
            let _window = app.get_webview_window("main").unwrap();
            #[cfg(debug_assertions)]
            {
                _window.open_devtools();
            }
            
            println!("Plume is starting...");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("ERROR: Failed to run Tauri application");
}
