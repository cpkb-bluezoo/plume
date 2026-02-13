/*
 * main.rs
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

// Disable the default Windows console window in release builds
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Import our modules
mod config;
mod crypto;
mod keys;
mod messages_store;
mod nostr;
mod relay;

// Import what we need from external crates
use tauri::{Emitter, Manager};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use qrcode::{QrCode, render::svg};

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
#[tauri::command(rename_all = "snake_case")]
fn fetch_notes_from_relays(
    relay_urls: Vec<String>,
    limit: u32,
    authors: Option<Vec<String>>,
    since: Option<u64>,
    profile_feed: Option<bool>,
) -> Result<String, String> {
    if relay_urls.is_empty() {
        return Err(String::from("No relays provided. Configure relays in Settings."));
    }
    let use_follows = authors.as_ref().map(|a| !a.is_empty()).unwrap_or(false);
    let is_profile_feed = profile_feed.unwrap_or(false);
    println!(
        "Fetching notes from {} relays (follows={}, since={:?}, profile_feed={})",
        relay_urls.len(),
        use_follows,
        since,
        is_profile_feed
    );

    let filter = if use_follows {
        if is_profile_feed {
            nostr::filter_profile_feed_by_authors_since(authors.unwrap(), limit, since)
        } else {
            nostr::filter_notes_by_authors_since(authors.unwrap(), limit, since)
        }
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
#[tauri::command(rename_all = "snake_case")]
fn start_feed_stream(
    app: tauri::AppHandle,
    relay_urls: Vec<String>,
    limit: u32,
    authors: Option<Vec<String>>,
    since: Option<u64>,
    stream_context: Option<String>,
) -> Result<(), String> {
    if relay_urls.is_empty() {
        return Err(String::from("No relays provided. Configure relays in Settings."));
    }
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
        if is_profile {
            nostr::filter_profile_feed_by_authors_since(
                authors.unwrap_or_default(),
                limit,
                since,
            )
        } else {
            nostr::filter_notes_by_authors_since(
                authors.unwrap_or_default(),
                limit,
                since,
            )
        }
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

// Tauri command: Fetch kind 1 notes that reference the given event_id in an "e" tag (replies to a note).
#[tauri::command(rename_all = "snake_case")]
fn fetch_replies_to_event(relay_urls: Vec<String>, event_id: String, limit: u32) -> Result<String, String> {
    if event_id.is_empty() {
        return Ok(String::from("[]"));
    }
    if relay_urls.is_empty() {
        return Err(String::from("No relays configured. Add relays in Settings."));
    }
    let filter = nostr::filter_replies_to_event(event_id, limit);
    let mut all_events: Vec<nostr::Event> = Vec::new();
    for relay_url in &relay_urls {
        match relay::fetch_notes_from_relay(relay_url, &filter, 10) {
            Ok(events) => {
                for event in events {
                    all_events.push(event);
                }
            }
            Err(e) => {
                println!("Error fetching replies from {}: {}", relay_url, e);
            }
        }
    }
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut unique: Vec<nostr::Event> = Vec::new();
    for event in all_events {
        if seen.insert(event.id.clone()) {
            unique.push(event);
        }
    }
    unique.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(events_to_json_array(&unique))
}

// Tauri command: Fetch events by IDs (e.g. for bookmarks page)
#[tauri::command(rename_all = "snake_case")]
fn fetch_events_by_ids(relay_urls: Vec<String>, ids: Vec<String>) -> Result<String, String> {
    if ids.is_empty() {
        return Ok(String::from("[]"));
    }
    if relay_urls.is_empty() {
        return Err(String::from("No relays configured. Add relays in Settings."));
    }
    let filter = nostr::filter_events_by_ids(ids);
    let mut all_events: Vec<nostr::Event> = Vec::new();
    for relay_url in relay_urls {
        match relay::fetch_notes_from_relay(&relay_url, &filter, 10) {
            Ok(events) => {
                for event in events {
                    all_events.push(event);
                }
            }
            Err(e) => {
                println!("Error fetching by ids from {}: {}", relay_url, e);
            }
        }
    }
    // Dedupe by event id (keep first occurrence)
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut unique: Vec<nostr::Event> = Vec::new();
    for event in all_events {
        if seen.insert(event.id.clone()) {
            unique.push(event);
        }
    }
    // Sort by created_at descending
    unique.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(events_to_json_array(&unique))
}

// Tauri command: Generate a QR code as SVG from a string (no external web service).
#[tauri::command(rename_all = "snake_case")]
fn generate_qr_svg(data: String) -> Result<String, String> {
    let code = QrCode::new(data.as_bytes()).map_err(|e| e.to_string())?;
    let svg_xml = code.render::<svg::Color<'_>>().build();
    Ok(svg_xml)
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
#[tauri::command(rename_all = "snake_case")]
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

// Tauri command: Follow or unfollow a user by updating kind 3 contact list. add: true = follow, false = unfollow.
#[tauri::command]
fn update_contact_list(
    state: tauri::State<AppState>,
    add: bool,
    target_pubkey: String,
) -> Result<String, String> {
    let config_dir = &state.config_dir;
    let cfg = match config::load_config(config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    let secret_key = match &cfg.private_key {
        Some(k) => k.as_str(),
        None => return Err(String::from("No private key configured. Add your nsec in Settings to follow users.")),
    };
    if cfg.public_key.is_empty() {
        return Err(String::from("No public key configured"));
    }
    let target_hex = keys::public_key_to_hex(&target_pubkey)
        .map_err(|e| format!("Invalid target pubkey: {}", e))?;
    let mut pubkeys: Vec<String> = match relay::fetch_following_from_relays(&cfg.relays, &cfg.public_key, 10) {
        Ok(Some(contact_list)) => nostr::get_following_pubkeys(&contact_list),
        Ok(None) => Vec::new(),
        Err(e) => return Err(format!("Failed to fetch current following: {}", e)),
    };
    if add {
        if !pubkeys.contains(&target_hex) {
            pubkeys.push(target_hex);
        }
    } else {
        pubkeys.retain(|p| p != &target_hex);
    }
    let event = crypto::create_signed_contact_list(&pubkeys, secret_key)?;
    let results = relay::publish_event_to_relays(&cfg.relays, &event, 10);
    let success_count = results.iter().filter(|r| r.success).count();
    if success_count == 0 {
        return Err(String::from("Failed to publish contact list to any relay"));
    }
    Ok(relay::publish_results_to_json(&results))
}

// Tauri command: Replace contact list with the given pubkeys (hex or npub). Publishes kind 3 to relays.
#[tauri::command(rename_all = "snake_case")]
fn set_contact_list(state: tauri::State<AppState>, pubkeys: Vec<String>) -> Result<String, String> {
    let config_dir = &state.config_dir;
    let cfg = match config::load_config(config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    let secret_key = match &cfg.private_key {
        Some(k) => k.as_str(),
        None => return Err(String::from("No private key configured. Add your nsec in Settings to follow users.")),
    };
    if cfg.public_key.is_empty() {
        return Err(String::from("No public key configured"));
    }
    let mut hex_pubkeys: Vec<String> = Vec::with_capacity(pubkeys.len());
    for p in &pubkeys {
        let hex = keys::public_key_to_hex(p).map_err(|e| format!("Invalid pubkey {}: {}", p, e))?;
        hex_pubkeys.push(hex);
    }
    let event = crypto::create_signed_contact_list(&hex_pubkeys, secret_key)?;
    let results = relay::publish_event_to_relays(&cfg.relays, &event, 10);
    let success_count = results.iter().filter(|r| r.success).count();
    if success_count == 0 {
        return Err(String::from("Failed to publish contact list to any relay"));
    }
    Ok(relay::publish_results_to_json(&results))
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

// Tauri command: Publish a reaction (like) to a note. NIP-25 kind 7. emoji defaults to "❤️" if empty.
#[tauri::command]
fn post_reaction(
    state: tauri::State<AppState>,
    event_id: String,
    author_pubkey: String,
    emoji: Option<String>,
) -> Result<String, String> {
    let config_dir = &state.config_dir;
    let cfg = match config::load_config(config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    let secret_key = match &cfg.private_key {
        Some(k) => k.as_str(),
        None => return Err(String::from("No private key configured. Add your nsec in Settings to react to notes.")),
    };
    if event_id.is_empty() || author_pubkey.is_empty() {
        return Err(String::from("event_id and author_pubkey are required"));
    }
    let content = emoji.as_deref().filter(|s| !s.is_empty()).unwrap_or("❤️");
    let event = crypto::create_signed_reaction(&event_id, &author_pubkey, content, secret_key)?;
    let results = relay::publish_event_to_relays(&cfg.relays, &event, 10);
    let success_count = results.iter().filter(|r| r.success).count();
    if success_count == 0 {
        return Err(String::from("Failed to publish reaction to any relay"));
    }
    Ok(relay::publish_results_to_json(&results))
}

// Tauri command: Publish a repost (kind 6) of a note. NIP-18. content_optional: stringified original event or empty.
#[tauri::command]
fn post_repost(
    state: tauri::State<AppState>,
    event_id: String,
    author_pubkey: String,
    content_optional: Option<String>,
) -> Result<String, String> {
    let config_dir = &state.config_dir;
    let cfg = match config::load_config(config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    let secret_key = match &cfg.private_key {
        Some(k) => k.as_str(),
        None => return Err(String::from("No private key configured. Add your nsec in Settings to repost.")),
    };
    if event_id.is_empty() || author_pubkey.is_empty() {
        return Err(String::from("event_id and author_pubkey are required"));
    }
    let content = content_optional.as_deref().unwrap_or("");
    let event = crypto::create_signed_repost(&event_id, &author_pubkey, content, secret_key)?;
    let results = relay::publish_event_to_relays(&cfg.relays, &event, 10);
    let success_count = results.iter().filter(|r| r.success).count();
    if success_count == 0 {
        return Err(String::from("Failed to publish repost to any relay"));
    }
    Ok(relay::publish_results_to_json(&results))
}

// ============================================================
// Direct Messages (NIP-04) Commands
// ============================================================

/// List conversations: JSON array of { other_pubkey, last_created_at }.
#[tauri::command]
fn get_conversations(state: tauri::State<AppState>) -> Result<String, String> {
    let config_dir = &state.config_dir;
    messages_store::ensure_messages_dir(config_dir).map_err(|e| e.to_string())?;
    messages_store::list_conversations_json(config_dir)
}

/// Get decrypted messages for a conversation. other_pubkey_hex: the other party's pubkey.
#[tauri::command(rename_all = "snake_case")]
fn get_messages(state: tauri::State<AppState>, other_pubkey_hex: String) -> Result<String, String> {
    let config_dir = &state.config_dir;
    let cfg = config::load_config(config_dir).map_err(|e| format!("Config: {}", e))?;
    let secret_hex = cfg
        .private_key
        .as_ref()
        .ok_or("No private key configured. Add your nsec in Settings to read messages.")?;
    let our_pubkey = keys::public_key_to_hex(&cfg.public_key).map_err(|e| format!("Public key: {}", e))?;
    let other_hex = keys::public_key_to_hex(other_pubkey_hex.trim()).map_err(|e| format!("Invalid other_pubkey: {}", e))?;
    let messages = messages_store::get_messages(config_dir, secret_hex, &our_pubkey, &other_hex)?;
    Ok(messages_store::messages_to_json(&messages))
}

/// Send a DM: encrypt, publish to relays, append to local store.
#[tauri::command(rename_all = "snake_case")]
fn send_dm(state: tauri::State<AppState>, recipient_pubkey: String, plaintext: String) -> Result<String, String> {
    let config_dir = &state.config_dir;
    messages_store::ensure_messages_dir(config_dir).map_err(|e| e.to_string())?;
    let cfg = config::load_config(config_dir).map_err(|e| format!("Config: {}", e))?;
    let secret_hex = cfg
        .private_key
        .as_ref()
        .ok_or("No private key configured. Add your nsec in Settings to send messages.")?;
    let recipient_hex = keys::public_key_to_hex(recipient_pubkey.trim()).map_err(|e| format!("Invalid recipient: {}", e))?;
    let event = crypto::create_signed_dm(&recipient_hex, &plaintext, secret_hex)?;
    let results = relay::publish_event_to_relays(&cfg.relays, &event, 10);
    let success_count = results.iter().filter(|r| r.success).count();
    if success_count == 0 {
        return Err(String::from("Failed to publish DM to any relay"));
    }
    let raw_json = nostr::event_to_json(&event);
    messages_store::append_raw_event(config_dir, &recipient_hex, &raw_json)
        .map_err(|e| format!("Published but failed to save locally: {}", e))?;
    Ok(nostr::event_to_json(&event))
}

/// Start long-lived DM subscription on all relays. On each kind 4 EVENT: merge into store, emit "dm-received".
#[tauri::command(rename_all = "snake_case")]
fn start_dm_stream(app: tauri::AppHandle, state: tauri::State<AppState>) -> Result<(), String> {
    let config_dir = state.config_dir.clone();
    let cfg = config::load_config(&config_dir).map_err(|e| format!("Config: {}", e))?;
    let our_pubkey_hex = keys::public_key_to_hex(cfg.public_key.trim()).map_err(|e| format!("Public key: {}", e))?;
    if our_pubkey_hex.is_empty() || cfg.relays.is_empty() {
        return Ok(());
    }
    let filter_received = nostr::filter_dms_received(&our_pubkey_hex, 500, None);
    let filter_sent = nostr::filter_dms_sent(&our_pubkey_hex, 500, None);

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(r) => r,
            Err(e) => {
                println!("DM stream: failed to create runtime: {}", e);
                return;
            }
        };
        rt.block_on(async move {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            for relay_url in &cfg.relays {
                let tx = tx.clone();
                let url = relay_url.clone();
                let f1 = filter_received.clone();
                let f2 = filter_sent.clone();
                tokio::spawn(async move {
                    relay::run_relay_dm_stream(url, f1, f2, tx).await;
                });
            }
            drop(tx);

            while let Some(msg) = rx.recv().await {
                match msg {
                    relay::StreamMessage::Event(event) => {
                        if let Some(other) = nostr::other_pubkey_in_dm(&event, &our_pubkey_hex) {
                            let raw = nostr::event_to_json(&event);
                            if let Err(e) = messages_store::append_raw_event(&config_dir, &other, &raw) {
                                println!("DM store append error: {}", e);
                            }
                            let _ = app.emit("dm-received", (other.clone(), raw));
                        }
                    }
                    _ => {}
                }
            }
        });
    });
    Ok(())
}

// Tauri command: Request a zap invoice (NIP-57). Resolves LUD16 to LNURL, builds signed zap request, returns { pr: "bolt11_invoice" }.
#[tauri::command]
fn request_zap_invoice(
    state: tauri::State<AppState>,
    target_lud16: String,
    amount_sats: u32,
    event_id: String,
    target_pubkey: String,
) -> Result<String, String> {
    let config_dir = &state.config_dir;
    let cfg = match config::load_config(config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    let secret_key = match &cfg.private_key {
        Some(k) => k.as_str(),
        None => return Err(String::from("No private key configured. Add your nsec in Settings to zap.")),
    };
    if target_lud16.is_empty() || target_pubkey.is_empty() {
        return Err(String::from("target_lud16 and target_pubkey are required"));
    }
    let amount_sats = if amount_sats >= 1 { amount_sats } else { 42 };
    let amount_msats: u64 = (amount_sats as u64) * 1000;

    // LUD16 "user@domain" -> https://domain/.well-known/lnurlp/user
    let lud16 = target_lud16.trim();
    let parts: Vec<&str> = lud16.splitn(2, '@').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(format!("Invalid Lightning address: {}", lud16));
    }
    let lnurl_user = parts[0];
    let domain = parts[1];
    let lnurl_url = format!("https://{}/.well-known/lnurlp/{}", domain, lnurl_user);

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client: {}", e))?;
    let resp = client.get(&lnurl_url).send().map_err(|e| format!("LNURL fetch: {}", e))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("LNURL endpoint returned {}", status));
    }
    let body = resp.text().map_err(|e| format!("LNURL response: {}", e))?;
    let lnurl_json = json::parse(&body).map_err(|e| format!("LNURL JSON: {}", e))?;
    let callback = match lnurl_json["callback"].as_str() {
        Some(s) => s.to_string(),
        None => return Err(String::from("LNURL response missing callback")),
    };
    let allows_nostr = lnurl_json["allowsNostr"].as_bool().unwrap_or(false);
    if !allows_nostr {
        return Err(String::from("Recipient does not support Nostr zaps (allowsNostr)"));
    }
    let min_sendable = lnurl_json["minSendable"].as_u64().unwrap_or(1000);
    let max_sendable = lnurl_json["maxSendable"].as_u64().unwrap_or(100_000_000);
    let amount_msats = amount_msats.clamp(min_sendable, max_sendable);

    let event_id_opt = if event_id.trim().is_empty() { None } else { Some(event_id.as_str()) };
    let zap_event = crypto::create_signed_zap_request(
        &cfg.relays,
        &target_pubkey,
        event_id_opt,
        amount_msats,
        "",
        secret_key,
    )?;
    let zap_json = nostr::event_to_json(&zap_event);
    let zap_b64 = BASE64.encode(zap_json.as_bytes());

    let sep = if callback.contains('?') { '&' } else { '?' };
    let callback_with_params = format!("{}{}amount={}&nostr={}", callback, sep, amount_msats, urlencoding::encode(&zap_b64));

    let resp2 = client.get(&callback_with_params).send().map_err(|e| format!("Callback fetch: {}", e))?;
    if !resp2.status().is_success() {
        return Err(format!("Zap callback returned {}", resp2.status()));
    }
    let body2 = resp2.text().map_err(|e| format!("Callback response: {}", e))?;
    let result_json = json::parse(&body2).map_err(|e| format!("Callback JSON: {}", e))?;
    let pr = match result_json["pr"].as_str() {
        Some(s) => s,
        None => return Err(String::from("Callback response missing pr (invoice)")),
    };
    let pr_escaped = pr.replace('\\', "\\\\").replace('"', "\\\"");
    Ok(format!(r#"{{"pr":"{}"}}"#, pr_escaped))
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
            media_server_url: String::from("https://blossom.primal.net"),
            muted_users: Vec::new(),
            muted_words: Vec::new(),
            muted_hashtags: Vec::new(),
            bookmarks: Vec::new(),
            default_zap_amount: 42,
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
    
    // Make sure the config and messages directories exist
    match config::ensure_config_dir(&config_dir) {
        Ok(()) => {
            println!("Config directory ready: {}", config_dir);
        }
        Err(e) => {
            eprintln!("ERROR: Could not create config directory: {}", e);
            std::process::exit(1);
        }
    }
    let _ = messages_store::ensure_messages_dir(&config_dir);
    
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
            fetch_events_by_ids,
            generate_qr_svg,
            fetch_replies_to_event,
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
            update_contact_list,
            set_contact_list,
            fetch_followers,
            fetch_own_followers,
            fetch_relay_list,
            // Posting / Signing commands
            post_note,
            post_reaction,
            post_repost,
            get_conversations,
            get_messages,
            send_dm,
            start_dm_stream,
            request_zap_invoice,
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
