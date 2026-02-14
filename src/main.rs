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
mod debug;
mod json;
mod keys;
mod messages_store;
mod nostr;
mod relay;
mod websocket;

// Import what we need from external crates
use tauri::{Emitter, Manager};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use qrcode::{QrCode, render::svg};

use std::sync::RwLock;

use bytes::BytesMut;
use crate::json::{JsonContentHandler, JsonNumber, JsonParser};

// Application state that persists while the app is running
struct AppState {
    base_dir: String,
    active_config_dir: RwLock<String>,
}

impl AppState {
    fn config_dir(&self) -> String {
        self.active_config_dir.read().unwrap().clone()
    }
    fn set_config_dir(&self, dir: String) {
        *self.active_config_dir.write().unwrap() = dir;
    }
}

// ============================================================
// Configuration Commands
// ============================================================

#[tauri::command]
fn get_config_dir(state: tauri::State<AppState>) -> String {
    state.config_dir()
}

#[tauri::command]
fn load_config(state: tauri::State<AppState>) -> Result<String, String> {
    let config_dir = state.config_dir();
    match config::load_config(&config_dir) {
        Ok(cfg) => {
            let json = config::config_to_json(&cfg);
            return Ok(json);
        }
        Err(e) => {
            return Err(format!("Failed to load config: {}", e));
        }
    }
}

#[tauri::command]
fn save_config(state: tauri::State<AppState>, config_json: String) -> Result<(), String> {
    let config_dir = state.config_dir();
    let mut cfg = match config::json_to_config(&config_json) {
        Ok(c) => c,
        Err(e) => return Err(format!("Invalid config JSON: {}", e)),
    };
    // Preserve existing profile fields if the incoming config doesn't set them
    if let Ok(existing) = config::load_config(&config_dir) {
        if cfg.name == "Anonymous" && existing.name != "Anonymous" {
            cfg.name = existing.name.clone();
        }
        if cfg.picture.is_none() && existing.picture.is_some() {
            cfg.picture = existing.picture.clone();
        }
        if cfg.about.is_none() && existing.about.is_some() {
            cfg.about = existing.about.clone();
        }
        if cfg.nip05.is_none() && existing.nip05.is_some() {
            cfg.nip05 = existing.nip05.clone();
        }
        if cfg.banner.is_none() && existing.banner.is_some() {
            cfg.banner = existing.banner.clone();
        }
        if cfg.website.is_none() && existing.website.is_some() {
            cfg.website = existing.website.clone();
        }
        if cfg.lud16.is_none() && existing.lud16.is_some() {
            cfg.lud16 = existing.lud16.clone();
        }
    }
    match config::save_config(&config_dir, &cfg) {
        Ok(()) => return Ok(()),
        Err(e) => return Err(format!("Failed to save config: {}", e)),
    }
}

// ============================================================
// Key Conversion Commands
// ============================================================

#[tauri::command]
fn convert_public_key_to_hex(key: String) -> Result<String, String> {
    return keys::public_key_to_hex(&key);
}

#[tauri::command]
fn convert_hex_to_npub(hex_key: String) -> Result<String, String> {
    return keys::hex_to_npub(&hex_key);
}

#[tauri::command]
fn convert_secret_key_to_hex(key: String) -> Result<String, String> {
    return keys::secret_key_to_hex(&key);
}

#[tauri::command]
fn convert_hex_to_nsec(hex_key: String) -> Result<String, String> {
    return keys::hex_to_nsec(&hex_key);
}

#[tauri::command]
fn parse_key(key: String) -> Result<String, String> {
    let trimmed = key.trim();
    
    if keys::is_npub(trimmed) {
        match keys::npub_to_hex(trimmed) {
            Ok(hex) => {
                return Ok(format!(
                    "{{\"type\":\"public\",\"hex\":\"{}\",\"npub\":\"{}\"}}",
                    hex, trimmed
                ));
            }
            Err(e) => return Err(format!("Invalid npub: {}", e)),
        }
    }
    
    if keys::is_nsec(trimmed) {
        match keys::nsec_to_hex(trimmed) {
            Ok(hex) => {
                return Ok(format!(
                    "{{\"type\":\"secret\",\"hex\":\"{}\",\"nsec\":\"{}\"}}",
                    hex, trimmed
                ));
            }
            Err(e) => return Err(format!("Invalid nsec: {}", e)),
        }
    }
    
    if keys::is_valid_hex_key(trimmed) {
        let hex = trimmed.to_lowercase();
        let npub = keys::hex_to_npub(&hex).unwrap_or_default();
        let nsec = keys::hex_to_nsec(&hex).unwrap_or_default();
        return Ok(format!(
            "{{\"type\":\"hex\",\"hex\":\"{}\",\"npub\":\"{}\",\"nsec\":\"{}\"}}",
            hex, npub, nsec
        ));
    }
    
    return Err(String::from("Invalid key format. Expected npub1..., nsec1..., or 64-char hex"));
}

// Decode a NIP-19 bech32 entity (nevent, nprofile, note, npub).
// Accepts the raw bech32 string (without the "nostr:" prefix).
#[tauri::command(rename_all = "snake_case")]
fn decode_nostr_uri(bech32_str: String) -> Result<String, String> {
    let trimmed = bech32_str.trim();

    if trimmed.starts_with("nevent1") {
        let decoded = keys::decode_nevent(trimmed)?;
        let mut json = String::from("{\"type\":\"nevent\",\"event_id\":\"");
        json.push_str(&decoded.event_id);
        json.push_str("\",\"relays\":[");
        for (i, r) in decoded.relays.iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            json.push('"');
            // Escape any quotes in relay URL
            for ch in r.chars() {
                if ch == '"' {
                    json.push_str("\\\"");
                } else if ch == '\\' {
                    json.push_str("\\\\");
                } else {
                    json.push(ch);
                }
            }
            json.push('"');
        }
        json.push_str("],\"author\":");
        match &decoded.author {
            Some(a) => { json.push('"'); json.push_str(a); json.push('"'); }
            None => json.push_str("null"),
        }
        json.push('}');
        return Ok(json);
    }

    if trimmed.starts_with("nprofile1") {
        let decoded = keys::decode_nprofile(trimmed)?;
        let mut json = String::from("{\"type\":\"nprofile\",\"pubkey\":\"");
        json.push_str(&decoded.pubkey);
        json.push_str("\",\"relays\":[");
        for (i, r) in decoded.relays.iter().enumerate() {
            if i > 0 {
                json.push(',');
            }
            json.push('"');
            for ch in r.chars() {
                if ch == '"' {
                    json.push_str("\\\"");
                } else if ch == '\\' {
                    json.push_str("\\\\");
                } else {
                    json.push(ch);
                }
            }
            json.push('"');
        }
        json.push_str("]}");
        return Ok(json);
    }

    if trimmed.starts_with("note1") {
        let event_id = keys::note_to_hex(trimmed)?;
        return Ok(format!("{{\"type\":\"note\",\"event_id\":\"{}\"}}", event_id));
    }

    if trimmed.starts_with("npub1") {
        let pubkey = keys::npub_to_hex(trimmed)?;
        return Ok(format!("{{\"type\":\"npub\",\"pubkey\":\"{}\"}}", pubkey));
    }

    Err(String::from("Unsupported NIP-19 entity. Expected nevent1..., nprofile1..., note1..., or npub1..."))
}

// ============================================================
// Relay Commands (all async)
// ============================================================

#[tauri::command]
async fn fetch_notes(relay_url: String, limit: u32) -> Result<String, String> {
    debug_log!("Fetching {} notes from {}", limit, relay_url);
    let filter = nostr::filter_recent_notes(limit);
    let events = relay::fetch_notes_from_relay(&relay_url, &filter, 10).await?;
    let json = events_to_json_array(&events);
    return Ok(json);
}

#[tauri::command(rename_all = "snake_case")]
async fn fetch_notes_from_relays(
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
    let relay_count = relay_urls.len();
    let mut fail_count: usize = 0;

    for relay_url in relay_urls {
        match relay::fetch_notes_from_relay(&relay_url, &filter, 10).await {
            Ok(events) => {
                for event in events {
                    all_events.push(event);
                }
            }
            Err(e) => {
                fail_count += 1;
                debug_log!("Error fetching from {}: {}", relay_url, e);
            }
        }
    }

    if fail_count == relay_count {
        return Err(format!(
            "Could not reach any of the {} configured relays. Check your connection and relay settings.",
            relay_count
        ));
    }

    all_events.sort_by(|a, b| b.created_at.cmp(&a.created_at));

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
    Ok(json)
}

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

    let filter = if use_follows {
        if is_profile {
            nostr::filter_profile_feed_by_authors_since(
                authors.unwrap_or_default(), limit, since,
            )
        } else {
            nostr::filter_notes_by_authors_since(
                authors.unwrap_or_default(), limit, since,
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
                warn_log!("Failed to create Tokio runtime: {}", e);
                return;
            }
        };
        rt.block_on(async move {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            for relay_url in relay_urls {
                let filter = filter.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    relay::run_relay_feed_stream(relay_url, filter, 10, tx).await;
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
                        debug_log!("Relay notice: {}", msg);
                    }
                }
            }
        });
    });

    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn fetch_replies_to_event(relay_urls: Vec<String>, event_id: String, limit: u32) -> Result<String, String> {
    if event_id.is_empty() {
        return Ok(String::from("[]"));
    }
    if relay_urls.is_empty() {
        return Err(String::from("No relays configured. Add relays in Settings."));
    }
    let filter = nostr::filter_replies_to_event(event_id, limit);
    let mut all_events: Vec<nostr::Event> = Vec::new();
    for relay_url in &relay_urls {
        match relay::fetch_notes_from_relay(relay_url, &filter, 10).await {
            Ok(events) => {
                for event in events {
                    all_events.push(event);
                }
            }
            Err(e) => {
                debug_log!("Error fetching replies from {}: {}", relay_url, e);
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

#[tauri::command(rename_all = "snake_case")]
async fn fetch_events_by_ids(relay_urls: Vec<String>, ids: Vec<String>) -> Result<String, String> {
    if ids.is_empty() {
        return Ok(String::from("[]"));
    }
    if relay_urls.is_empty() {
        return Err(String::from("No relays configured. Add relays in Settings."));
    }
    let filter = nostr::filter_events_by_ids(ids);
    let mut all_events: Vec<nostr::Event> = Vec::new();
    for relay_url in relay_urls {
        match relay::fetch_notes_from_relay(&relay_url, &filter, 10).await {
            Ok(events) => {
                for event in events {
                    all_events.push(event);
                }
            }
            Err(e) => {
                debug_log!("Error fetching by ids from {}: {}", relay_url, e);
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
    unique.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(events_to_json_array(&unique))
}

#[tauri::command(rename_all = "snake_case")]
fn generate_qr_svg(data: String) -> Result<String, String> {
    let code = QrCode::new(data.as_bytes()).map_err(|e| e.to_string())?;
    let svg_xml = code.render::<svg::Color<'_>>().build();
    Ok(svg_xml)
}

#[tauri::command]
async fn test_relay_connection(relay_url: String) -> Result<String, String> {
    debug_log!("Testing connection to: {}", relay_url);
    // Explicit user test — bypass backoff, but clear it on success
    match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        crate::websocket::WebSocketClient::connect(&relay_url),
    ).await {
        Ok(Ok(_conn)) => {
            relay::record_relay_success(&relay_url);
            Ok(String::from("Connection successful"))
        }
        Ok(Err(e)) => Err(format!("Failed to connect: {}", e)),
        Err(_) => Err(String::from("Connection timeout")),
    }
}

/// Return backoff status for a list of relay URLs.
/// Returns JSON: {"wss://relay.example.com": 42, ...}  (remaining seconds, absent = not in backoff)
#[tauri::command(rename_all = "snake_case")]
fn get_relay_backoff_status(relay_urls: Vec<String>) -> String {
    let mut json = String::from("{");
    let mut first = true;
    for url in &relay_urls {
        if let Some(remaining) = relay::check_relay_backoff(url) {
            if !first {
                json.push(',');
            }
            json.push('"');
            json.push_str(&config::escape_json_string(url));
            json.push_str("\":");
            json.push_str(&remaining.to_string());
            first = false;
        }
    }
    json.push('}');
    json
}

// ============================================================
// Profile Commands
// ============================================================

#[tauri::command(rename_all = "snake_case")]
async fn fetch_profile(pubkey: String, relay_urls: Vec<String>) -> Result<String, String> {
    let hex_pubkey = match keys::public_key_to_hex(&pubkey) {
        Ok(hex) => hex,
        Err(e) => return Err(format!("Invalid public key: {}", e)),
    };
    match relay::fetch_profile_from_relays(&relay_urls, &hex_pubkey, 5).await {
        Ok(Some(profile)) => Ok(nostr::profile_to_json(&profile)),
        Ok(None) => Ok(String::from("{}")),
        Err(e) => Err(format!("Failed to fetch profile: {}", e)),
    }
}

#[tauri::command]
async fn fetch_own_profile(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let config_dir = state.config_dir();
    let cfg = match config::load_config(&config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    if cfg.public_key.is_empty() {
        return Err(String::from("No public key configured"));
    }
    match relay::fetch_profile_from_relays(&cfg.relays, &cfg.public_key, 5).await {
        Ok(Some(profile)) => Ok(nostr::profile_to_json(&profile)),
        Ok(None) => Ok(String::from("{}")),
        Err(e) => Err(format!("Failed to fetch profile: {}", e)),
    }
}

// ============================================================
// Verification Commands
// ============================================================

#[tauri::command]
fn verify_event(event_json: String) -> Result<String, String> {
    let event = match nostr::parse_event(&event_json) {
        Ok(e) => e,
        Err(e) => return Err(format!("Invalid event JSON: {}", e)),
    };
    let result = crypto::verify_event(&event)?;
    return Ok(crypto::verification_result_to_json(&result));
}

#[tauri::command]
fn verify_event_id(event_json: String) -> Result<bool, String> {
    let event = match nostr::parse_event(&event_json) {
        Ok(e) => e,
        Err(e) => return Err(format!("Invalid event JSON: {}", e)),
    };
    return crypto::verify_event_id(&event);
}

#[tauri::command]
fn verify_event_signature(event_json: String) -> Result<bool, String> {
    let event = match nostr::parse_event(&event_json) {
        Ok(e) => e,
        Err(e) => return Err(format!("Invalid event JSON: {}", e)),
    };
    return crypto::verify_event_signature(&event);
}

#[tauri::command]
fn compute_event_id(event_json: String) -> Result<String, String> {
    let event = match nostr::parse_event(&event_json) {
        Ok(e) => e,
        Err(e) => return Err(format!("Invalid event JSON: {}", e)),
    };
    return crypto::compute_event_id(&event);
}

// ============================================================
// Following / Followers Commands
// ============================================================

#[tauri::command]
async fn fetch_following(pubkey: String, relay_urls: Vec<String>) -> Result<String, String> {
    let hex_pubkey = match keys::public_key_to_hex(&pubkey) {
        Ok(hex) => hex,
        Err(e) => return Err(format!("Invalid public key: {}", e)),
    };
    match relay::fetch_following_from_relays(&relay_urls, &hex_pubkey, 10).await {
        Ok(Some(contact_list)) => Ok(nostr::contact_list_to_json(&contact_list)),
        Ok(None) => Ok(String::from("{\"owner_pubkey\":\"\",\"created_at\":0,\"count\":0,\"contacts\":[]}")),
        Err(e) => Err(format!("Failed to fetch following: {}", e)),
    }
}

#[tauri::command]
async fn fetch_own_following(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let config_dir = state.config_dir();
    let cfg = match config::load_config(&config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    if cfg.public_key.is_empty() {
        return Err(String::from("No public key configured"));
    }
    match relay::fetch_following_from_relays(&cfg.relays, &cfg.public_key, 10).await {
        Ok(Some(contact_list)) => {
            // Sync the following list to local config for fast access by the feed
            let pubkeys = nostr::get_following_pubkeys(&contact_list);
            if !pubkeys.is_empty() {
                let mut cfg = cfg;
                cfg.following = pubkeys;
                if let Err(e) = config::save_config(&config_dir, &cfg) {
                    debug_log!("Warning: failed to cache following list locally: {}", e);
                }
            }
            Ok(nostr::contact_list_to_json(&contact_list))
        },
        Ok(None) => Ok(String::from("{\"owner_pubkey\":\"\",\"created_at\":0,\"count\":0,\"contacts\":[]}")),
        Err(e) => Err(format!("Failed to fetch following: {}", e)),
    }
}

#[tauri::command]
async fn update_contact_list(
    state: tauri::State<'_, AppState>,
    add: bool,
    target_pubkey: String,
) -> Result<String, String> {
    let config_dir = state.config_dir();
    let cfg = match config::load_config(&config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    let secret_key = match &cfg.private_key {
        Some(k) => k.clone(),
        None => return Err(String::from("No private key configured. Add your nsec in Settings to follow users.")),
    };
    if cfg.public_key.is_empty() {
        return Err(String::from("No public key configured"));
    }
    let target_hex = keys::public_key_to_hex(&target_pubkey)
        .map_err(|e| format!("Invalid target pubkey: {}", e))?;
    let mut pubkeys: Vec<String> = match relay::fetch_following_from_relays(&cfg.relays, &cfg.public_key, 10).await {
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
    let event = crypto::create_signed_contact_list(&pubkeys, &secret_key)?;
    let results = relay::publish_event_to_relays(&cfg.relays, &event, 10).await;
    let success_count = results.iter().filter(|r| r.success).count();
    if success_count == 0 {
        return Err(String::from("Failed to publish contact list to any relay"));
    }
    // Persist following list locally
    let mut cfg = cfg;
    cfg.following = pubkeys;
    if let Err(e) = config::save_config(&config_dir, &cfg) {
        warn_log!("Warning: published contact list but failed to save locally: {}", e);
    }
    Ok(relay::publish_results_to_json(&results))
}

#[tauri::command(rename_all = "snake_case")]
async fn set_contact_list(state: tauri::State<'_, AppState>, pubkeys: Vec<String>) -> Result<String, String> {
    let config_dir = state.config_dir();
    let mut cfg = match config::load_config(&config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    let secret_key = match &cfg.private_key {
        Some(k) => k.clone(),
        None => return Err(String::from("No private key configured.")),
    };
    if cfg.public_key.is_empty() {
        return Err(String::from("No public key configured"));
    }
    let mut hex_pubkeys: Vec<String> = Vec::with_capacity(pubkeys.len());
    for p in &pubkeys {
        let hex = keys::public_key_to_hex(p).map_err(|e| format!("Invalid pubkey {}: {}", p, e))?;
        hex_pubkeys.push(hex);
    }
    let event = crypto::create_signed_contact_list(&hex_pubkeys, &secret_key)?;
    let results = relay::publish_event_to_relays(&cfg.relays, &event, 10).await;
    let success_count = results.iter().filter(|r| r.success).count();
    if success_count == 0 {
        return Err(String::from("Failed to publish contact list to any relay"));
    }
    // Persist following list locally so the feed can use it without fetching from relays
    cfg.following = hex_pubkeys;
    if let Err(e) = config::save_config(&config_dir, &cfg) {
        warn_log!("Warning: published contact list but failed to save locally: {}", e);
    }
    Ok(relay::publish_results_to_json(&results))
}

#[tauri::command]
async fn fetch_followers(pubkey: String, relay_urls: Vec<String>) -> Result<String, String> {
    let hex_pubkey = match keys::public_key_to_hex(&pubkey) {
        Ok(hex) => hex,
        Err(e) => return Err(format!("Invalid public key: {}", e)),
    };
    match relay::fetch_followers_from_relays(&relay_urls, &hex_pubkey, 10).await {
        Ok(followers) => Ok(nostr::followers_to_json(&followers)),
        Err(e) => Err(format!("Failed to fetch followers: {}", e)),
    }
}

#[tauri::command]
async fn fetch_own_followers(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let config_dir = state.config_dir();
    let cfg = match config::load_config(&config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    if cfg.public_key.is_empty() {
        return Err(String::from("No public key configured"));
    }
    match relay::fetch_followers_from_relays(&cfg.relays, &cfg.public_key, 10).await {
        Ok(followers) => Ok(nostr::followers_to_json(&followers)),
        Err(e) => Err(format!("Failed to fetch followers: {}", e)),
    }
}

#[tauri::command]
async fn fetch_relay_list(pubkey: String, relay_urls: Vec<String>) -> Result<String, String> {
    let hex_pubkey = match keys::public_key_to_hex(&pubkey) {
        Ok(hex) => hex,
        Err(e) => return Err(format!("Invalid public key: {}", e)),
    };
    match relay::fetch_relay_list_from_relays(&relay_urls, &hex_pubkey, 10).await {
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

#[tauri::command]
async fn post_note(
    state: tauri::State<'_, AppState>,
    content: String,
    reply_to_event_id: Option<String>,
    reply_to_pubkey: Option<String>,
) -> Result<String, String> {
    let config_dir = state.config_dir();
    let cfg = match config::load_config(&config_dir) {
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
    let results = relay::publish_event_to_relays(&cfg.relays, &event, 10).await;
    let success_count = results.iter().filter(|r| r.success).count();
    if success_count == 0 {
        return Err(String::from("Failed to publish to any relay"));
    }
    Ok(relay::publish_results_to_json(&results))
}

#[tauri::command]
async fn post_reaction(
    state: tauri::State<'_, AppState>,
    event_id: String,
    author_pubkey: String,
    emoji: Option<String>,
) -> Result<String, String> {
    let config_dir = state.config_dir();
    let cfg = match config::load_config(&config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    let secret_key = match &cfg.private_key {
        Some(k) => k.clone(),
        None => return Err(String::from("No private key configured.")),
    };
    if event_id.is_empty() || author_pubkey.is_empty() {
        return Err(String::from("event_id and author_pubkey are required"));
    }
    let content = emoji.as_deref().filter(|s| !s.is_empty()).unwrap_or("❤️");
    let event = crypto::create_signed_reaction(&event_id, &author_pubkey, content, &secret_key)?;
    let results = relay::publish_event_to_relays(&cfg.relays, &event, 10).await;
    let success_count = results.iter().filter(|r| r.success).count();
    if success_count == 0 {
        return Err(String::from("Failed to publish reaction to any relay"));
    }
    Ok(relay::publish_results_to_json(&results))
}

#[tauri::command]
async fn post_repost(
    state: tauri::State<'_, AppState>,
    event_id: String,
    author_pubkey: String,
    content_optional: Option<String>,
) -> Result<String, String> {
    let config_dir = state.config_dir();
    let cfg = match config::load_config(&config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    let secret_key = match &cfg.private_key {
        Some(k) => k.clone(),
        None => return Err(String::from("No private key configured.")),
    };
    if event_id.is_empty() || author_pubkey.is_empty() {
        return Err(String::from("event_id and author_pubkey are required"));
    }
    let content = content_optional.as_deref().unwrap_or("");
    let event = crypto::create_signed_repost(&event_id, &author_pubkey, content, &secret_key)?;
    let results = relay::publish_event_to_relays(&cfg.relays, &event, 10).await;
    let success_count = results.iter().filter(|r| r.success).count();
    if success_count == 0 {
        return Err(String::from("Failed to publish repost to any relay"));
    }
    Ok(relay::publish_results_to_json(&results))
}

// ============================================================
// Direct Messages (NIP-04) Commands
// ============================================================

#[tauri::command]
fn get_conversations(state: tauri::State<AppState>) -> Result<String, String> {
    let config_dir = state.config_dir();
    messages_store::ensure_messages_dir(&config_dir).map_err(|e| e.to_string())?;
    messages_store::list_conversations_json(&config_dir)
}

/// Count conversations with unread messages (messages newer than dm_last_read_at).
#[tauri::command]
fn count_unread_dms(state: tauri::State<AppState>) -> Result<u32, String> {
    let config_dir = state.config_dir();
    let cfg = config::load_config(&config_dir).map_err(|e| format!("Config: {}", e))?;
    Ok(messages_store::count_unread_conversations(&config_dir, cfg.dm_last_read_at))
}

/// Mark DMs as read by updating dm_last_read_at to the current time.
#[tauri::command]
fn mark_dms_read(state: tauri::State<AppState>) -> Result<(), String> {
    let config_dir = state.config_dir();
    let mut cfg = config::load_config(&config_dir).map_err(|e| format!("Config: {}", e))?;
    cfg.dm_last_read_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    config::save_config(&config_dir, &cfg)
}

#[tauri::command(rename_all = "snake_case")]
fn get_messages(state: tauri::State<AppState>, other_pubkey_hex: String) -> Result<String, String> {
    let config_dir = state.config_dir();
    let cfg = config::load_config(&config_dir).map_err(|e| format!("Config: {}", e))?;
    let secret_hex = cfg.private_key.as_ref()
        .ok_or("No private key configured. Add your nsec in Settings to read messages.")?;
    let our_pubkey = keys::public_key_to_hex(&cfg.public_key).map_err(|e| format!("Public key: {}", e))?;
    let other_hex = keys::public_key_to_hex(other_pubkey_hex.trim()).map_err(|e| format!("Invalid other_pubkey: {}", e))?;
    let messages = messages_store::get_messages(&config_dir, secret_hex, &our_pubkey, &other_hex)?;
    Ok(messages_store::messages_to_json(&messages))
}

#[tauri::command(rename_all = "snake_case")]
async fn send_dm(state: tauri::State<'_, AppState>, recipient_pubkey: String, plaintext: String) -> Result<String, String> {
    let config_dir = state.config_dir();
    messages_store::ensure_messages_dir(&config_dir).map_err(|e| e.to_string())?;
    let cfg = config::load_config(&config_dir).map_err(|e| format!("Config: {}", e))?;
    let secret_hex = cfg.private_key.as_ref()
        .ok_or("No private key configured.")?
        .clone();
    let recipient_hex = keys::public_key_to_hex(recipient_pubkey.trim()).map_err(|e| format!("Invalid recipient: {}", e))?;
    let event = crypto::create_signed_dm(&recipient_hex, &plaintext, &secret_hex)?;
    let results = relay::publish_event_to_relays(&cfg.relays, &event, 10).await;
    let success_count = results.iter().filter(|r| r.success).count();
    if success_count == 0 {
        return Err(String::from("Failed to publish DM to any relay"));
    }
    let raw_json = nostr::event_to_json(&event);
    messages_store::append_raw_event(&config_dir, &recipient_hex, &raw_json)
        .map_err(|e| format!("Published but failed to save locally: {}", e))?;
    Ok(nostr::event_to_json(&event))
}

#[tauri::command(rename_all = "snake_case")]
fn start_dm_stream(app: tauri::AppHandle, state: tauri::State<AppState>) -> Result<(), String> {
    let config_dir = state.config_dir();
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
                warn_log!("DM stream: failed to create runtime: {}", e);
                return;
            }
        };
        rt.block_on(async move {
            let num_relays = cfg.relays.len() as u32;
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

            let mut eose_count = 0u32;
            let mut initial_sync = true;

            while let Some(msg) = rx.recv().await {
                match msg {
                    relay::StreamMessage::Event(event) => {
                        if let Some(other) = nostr::other_pubkey_in_dm(&event, &our_pubkey_hex) {
                            let raw = nostr::event_to_json(&event);
                            match messages_store::append_raw_event(&config_dir, &other, &raw) {
                                Ok(true) => {
                                    if initial_sync {
                                        // During initial sync, don't emit per-event notifications.
                                        // The frontend will re-count unread after dm-sync-done.
                                    } else {
                                        // Live message — notify frontend
                                        let _ = app.emit("dm-received", (other.clone(), raw));
                                    }
                                }
                                Ok(false) => {
                                    // Duplicate from another relay — skip emit
                                }
                                Err(e) => {
                                    warn_log!("DM store append error: {}", e);
                                }
                            }
                        }
                    }
                    relay::StreamMessage::Eose => {
                        eose_count += 1;
                        if initial_sync && eose_count >= num_relays {
                            initial_sync = false;
                            // Tell the frontend the initial DM sync is complete
                            let _ = app.emit("dm-sync-done", ());
                        }
                    }
                    _ => {}
                }
            }
        });
    });
    Ok(())
}

// ============================================================
// Zap Invoice (NIP-57) -- uses push JSON parser for LNURL responses
// ============================================================

/// Handler for LNURL response JSON: extracts callback, allowsNostr, minSendable, maxSendable.
struct LnurlResponseHandler {
    current_field: Option<String>,
    callback: Option<String>,
    allows_nostr: bool,
    min_sendable: u64,
    max_sendable: u64,
}

impl LnurlResponseHandler {
    fn new() -> Self {
        Self {
            current_field: None,
            callback: None,
            allows_nostr: false,
            min_sendable: 1000,
            max_sendable: 100_000_000,
        }
    }
}

impl JsonContentHandler for LnurlResponseHandler {
    fn start_object(&mut self) {}
    fn end_object(&mut self) {}
    fn start_array(&mut self) {}
    fn end_array(&mut self) {}
    fn key(&mut self, key: &str) {
        self.current_field = Some(key.to_string());
    }
    fn string_value(&mut self, value: &str) {
        if let Some(ref f) = self.current_field {
            if f == "callback" {
                self.callback = Some(value.to_string());
            }
        }
    }
    fn number_value(&mut self, number: JsonNumber) {
        if let Some(ref f) = self.current_field {
            match f.as_str() {
                "minSendable" => self.min_sendable = number.as_f64().max(0.0) as u64,
                "maxSendable" => self.max_sendable = number.as_f64().max(0.0) as u64,
                _ => {}
            }
        }
    }
    fn boolean_value(&mut self, value: bool) {
        if let Some(ref f) = self.current_field {
            if f == "allowsNostr" {
                self.allows_nostr = value;
            }
        }
    }
    fn null_value(&mut self) {}
}

/// Handler for zap callback response: extracts pr (bolt11 invoice).
struct ZapCallbackHandler {
    current_field: Option<String>,
    pr: Option<String>,
}

impl ZapCallbackHandler {
    fn new() -> Self {
        Self { current_field: None, pr: None }
    }
}

impl JsonContentHandler for ZapCallbackHandler {
    fn start_object(&mut self) {}
    fn end_object(&mut self) {}
    fn start_array(&mut self) {}
    fn end_array(&mut self) {}
    fn key(&mut self, key: &str) {
        self.current_field = Some(key.to_string());
    }
    fn string_value(&mut self, value: &str) {
        if let Some(ref f) = self.current_field {
            if f == "pr" {
                self.pr = Some(value.to_string());
            }
        }
    }
    fn number_value(&mut self, _number: JsonNumber) {}
    fn boolean_value(&mut self, _value: bool) {}
    fn null_value(&mut self) {}
}

fn parse_json_with_handler<H: JsonContentHandler>(body: &str, handler: &mut H) -> Result<(), String> {
    let mut parser = JsonParser::new();
    let mut buf = BytesMut::from(body.as_bytes());
    parser.receive(&mut buf, handler).map_err(|e| format!("JSON parse error: {}", e))?;
    parser.close(handler).map_err(|e| format!("JSON parse error: {}", e))?;
    Ok(())
}

#[tauri::command]
async fn request_zap_invoice(
    state: tauri::State<'_, AppState>,
    target_lud16: String,
    amount_sats: u32,
    event_id: String,
    target_pubkey: String,
) -> Result<String, String> {
    let config_dir = state.config_dir();
    let cfg = match config::load_config(&config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    let secret_key = match &cfg.private_key {
        Some(k) => k.clone(),
        None => return Err(String::from("No private key configured.")),
    };
    if target_lud16.is_empty() || target_pubkey.is_empty() {
        return Err(String::from("target_lud16 and target_pubkey are required"));
    }
    let amount_sats = if amount_sats >= 1 { amount_sats } else { 42 };
    let amount_msats: u64 = (amount_sats as u64) * 1000;

    let lud16 = target_lud16.trim();
    let parts: Vec<&str> = lud16.splitn(2, '@').collect();
    if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
        return Err(format!("Invalid Lightning address: {}", lud16));
    }
    let lnurl_user = parts[0];
    let domain = parts[1];
    let lnurl_url = format!("https://{}/.well-known/lnurlp/{}", domain, lnurl_user);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("HTTP client: {}", e))?;
    let resp = client.get(&lnurl_url).send().await.map_err(|e| format!("LNURL fetch: {}", e))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("LNURL endpoint returned {}", status));
    }
    let body = resp.text().await.map_err(|e| format!("LNURL response: {}", e))?;

    let mut lnurl_handler = LnurlResponseHandler::new();
    parse_json_with_handler(&body, &mut lnurl_handler)?;

    let callback = lnurl_handler.callback.ok_or("LNURL response missing callback")?;
    if !lnurl_handler.allows_nostr {
        return Err(String::from("Recipient does not support Nostr zaps (allowsNostr)"));
    }
    let amount_msats = amount_msats.clamp(lnurl_handler.min_sendable, lnurl_handler.max_sendable);

    let event_id_opt = if event_id.trim().is_empty() { None } else { Some(event_id.as_str()) };
    let zap_event = crypto::create_signed_zap_request(
        &cfg.relays,
        &target_pubkey,
        event_id_opt,
        amount_msats,
        "",
        &secret_key,
    )?;
    let zap_json = nostr::event_to_json(&zap_event);
    let zap_b64 = BASE64.encode(zap_json.as_bytes());

    let sep = if callback.contains('?') { '&' } else { '?' };
    let callback_with_params = format!("{}{}amount={}&nostr={}", callback, sep, amount_msats, urlencoding::encode(&zap_b64));

    let resp2 = client.get(&callback_with_params).send().await.map_err(|e| format!("Callback fetch: {}", e))?;
    if !resp2.status().is_success() {
        return Err(format!("Zap callback returned {}", resp2.status()));
    }
    let body2 = resp2.text().await.map_err(|e| format!("Callback response: {}", e))?;

    let mut zap_handler = ZapCallbackHandler::new();
    parse_json_with_handler(&body2, &mut zap_handler)?;

    let pr = zap_handler.pr.ok_or("Callback response missing pr (invoice)")?;
    let pr_escaped = pr.replace('\\', "\\\\").replace('"', "\\\"");
    Ok(format!(r#"{{"pr":"{}"}}"#, pr_escaped))
}

// ============================================================
// Profile Metadata
// ============================================================

#[tauri::command]
async fn set_profile_metadata(state: tauri::State<'_, AppState>, profile_json: String) -> Result<String, String> {
    let config_dir = state.config_dir();
    let mut cfg = match config::load_config(&config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    let secret_key = match cfg.private_key.as_ref() {
        Some(k) => k.clone(),
        None => return Err(String::from("No private key configured.")),
    };
    if cfg.relays.is_empty() {
        return Err(String::from("No relays configured"));
    }
    let profile = match nostr::parse_profile(&profile_json) {
        Ok(p) => p,
        Err(e) => return Err(format!("Invalid profile JSON: {}", e)),
    };
    let content = nostr::profile_to_content(&profile);
    let event = match crypto::create_signed_metadata_event(&content, &secret_key) {
        Ok(e) => e,
        Err(e) => return Err(format!("Failed to create profile event: {}", e)),
    };
    let results = relay::publish_event_to_relays(&cfg.relays, &event, 10).await;
    let success_count = results.iter().filter(|r| r.success).count();
    if success_count == 0 {
        return Err(String::from("Failed to publish profile to any relay"));
    }
    if let Some(ref name) = profile.name {
        cfg.name = name.clone();
    }
    cfg.about = profile.about.clone();
    cfg.picture = profile.picture.clone();
    cfg.nip05 = profile.nip05.clone();
    cfg.banner = profile.banner.clone();
    cfg.website = profile.website.clone();
    cfg.lud16 = profile.lud16.clone();
    if let Err(e) = config::save_config(&config_dir, &cfg) {
        return Err(format!("Profile published but failed to save local config: {}", e));
    }
    Ok(relay::publish_results_to_json(&results))
}

#[tauri::command]
fn sign_event(state: tauri::State<AppState>, event_json: String) -> Result<String, String> {
    let config_dir = state.config_dir();
    let cfg = match config::load_config(&config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    let secret_key = match cfg.private_key {
        Some(key) => key,
        None => return Err(String::from("No private key configured")),
    };
    let mut event = match nostr::parse_event(&event_json) {
        Ok(e) => e,
        Err(e) => return Err(format!("Invalid event JSON: {}", e)),
    };
    match crypto::sign_event(&mut event, &secret_key) {
        Ok(()) => {}
        Err(e) => return Err(format!("Failed to sign event: {}", e)),
    };
    return Ok(nostr::event_to_json(&event));
}

#[tauri::command]
fn get_derived_public_key(state: tauri::State<AppState>) -> Result<String, String> {
    let config_dir = state.config_dir();
    let cfg = match config::load_config(&config_dir) {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to load config: {}", e)),
    };
    let secret_key = match cfg.private_key {
        Some(key) => key,
        None => return Err(String::from("No private key configured")),
    };
    let pubkey = crypto::get_public_key_from_secret(&secret_key)?;
    let npub = keys::hex_to_npub(&pubkey).unwrap_or_default();
    Ok(format!("{{\"hex\":\"{}\",\"npub\":\"{}\"}}", pubkey, npub))
}

#[tauri::command]
fn generate_keypair(state: tauri::State<AppState>) -> Result<String, String> {
    let (secret_hex, pubkey_hex) = crypto::generate_keypair()?;
    let npub = keys::hex_to_npub(&pubkey_hex).unwrap_or_default();
    let nsec = keys::hex_to_nsec(&secret_hex).unwrap_or_default();

    // Create profile directory for the new identity
    let profile_dir = config::ensure_profile_dir(&state.base_dir, &npub)?;

    let mut cfg = config::Config::new();
    cfg.public_key = pubkey_hex.clone();
    cfg.private_key = Some(secret_hex.clone());
    config::save_config(&profile_dir, &cfg)?;

    // Update app config — use unwrap_or for missing file (Ok path), propagate parse errors
    let mut app_config = config::load_app_config(&state.base_dir)?;
    app_config.active_profile = Some(npub.clone());
    if !app_config.known_profiles.iter().any(|p| p == &npub) {
        app_config.known_profiles.push(npub.clone());
    }
    config::save_app_config(&state.base_dir, &app_config)?;

    // Switch to the new profile
    state.set_config_dir(profile_dir.clone());
    let _ = messages_store::ensure_messages_dir(&profile_dir);

    Ok(format!(
        "{{\"public_key_hex\":\"{}\",\"private_key_hex\":\"{}\",\"npub\":\"{}\",\"nsec\":\"{}\"}}",
        pubkey_hex, secret_hex, npub, nsec
    ))
}

// ============================================================
// Multi-Profile / Auth Commands
// ============================================================

#[tauri::command]
fn get_app_config(state: tauri::State<AppState>) -> Result<String, String> {
    config::load_app_config(&state.base_dir)
        .map(|c| config::app_config_to_json(&c))
}

#[tauri::command(rename_all = "snake_case")]
fn login_with_keys(
    state: tauri::State<AppState>,
    public_key: String,
    private_key: Option<String>,
) -> Result<String, String> {
    // Validate and convert public key
    let pub_hex = keys::public_key_to_hex(&public_key)
        .map_err(|e| format!("Invalid public key: {}", e))?;
    let npub = keys::hex_to_npub(&pub_hex)
        .map_err(|e| format!("Failed to convert to npub: {}", e))?;

    // Validate private key if provided
    let priv_hex = match &private_key {
        Some(key) if !key.trim().is_empty() => {
            Some(keys::secret_key_to_hex(key.trim())
                .map_err(|e| format!("Invalid private key: {}", e))?)
        }
        _ => None,
    };

    // Create profile directory
    let profile_dir = config::ensure_profile_dir(&state.base_dir, &npub)?;

    // Load existing config or create new
    let mut cfg = match config::load_config(&profile_dir) {
        Ok(c) => c,
        Err(_) => config::Config::new(),
    };
    cfg.public_key = pub_hex;
    if let Some(ref hex) = priv_hex {
        cfg.private_key = Some(hex.clone());
    }
    config::save_config(&profile_dir, &cfg)?;

    // Update app config — propagate errors instead of silently creating empty config
    let mut app_config = config::load_app_config(&state.base_dir)?;
    app_config.active_profile = Some(npub.clone());
    if !app_config.known_profiles.iter().any(|p| p == &npub) {
        app_config.known_profiles.push(npub.clone());
    }
    config::save_app_config(&state.base_dir, &app_config)?;

    // Switch to this profile
    state.set_config_dir(profile_dir.clone());
    let _ = messages_store::ensure_messages_dir(&profile_dir);

    Ok(config::config_to_json(&cfg))
}

#[tauri::command(rename_all = "snake_case")]
fn switch_profile(state: tauri::State<AppState>, npub: String) -> Result<String, String> {
    let profile_dir = config::get_profile_dir(&state.base_dir, &npub);
    if !std::path::Path::new(&profile_dir).join("config.json").exists() {
        return Err(format!("Profile not found: {}", npub));
    }

    let cfg = config::load_config(&profile_dir)?;

    let mut app_config = config::load_app_config(&state.base_dir)?;
    app_config.active_profile = Some(npub.clone());
    if !app_config.known_profiles.iter().any(|p| p == &npub) {
        app_config.known_profiles.push(npub.clone());
    }
    config::save_app_config(&state.base_dir, &app_config)?;

    state.set_config_dir(profile_dir.clone());
    let _ = messages_store::ensure_messages_dir(&profile_dir);

    Ok(config::config_to_json(&cfg))
}

#[tauri::command]
fn logout(state: tauri::State<AppState>) -> Result<(), String> {
    let mut app_config = config::load_app_config(&state.base_dir)?;
    debug_log!("[logout] Loaded app config: active_profile={:?}, known_profiles={:?}",
        app_config.active_profile, app_config.known_profiles);
    app_config.active_profile = None;
    config::save_app_config(&state.base_dir, &app_config)?;
    debug_log!("[logout] Saved app config, known_profiles preserved: {:?}", app_config.known_profiles);
    state.set_config_dir(state.base_dir.clone());
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
fn delete_profile(state: tauri::State<AppState>, npub: String) -> Result<(), String> {
    let profile_dir = config::get_profile_dir(&state.base_dir, &npub);
    if std::path::Path::new(&profile_dir).exists() {
        std::fs::remove_dir_all(&profile_dir)
            .map_err(|e| format!("Failed to delete profile directory: {}", e))?;
    }
    let mut app_config = config::load_app_config(&state.base_dir)?;
    app_config.known_profiles.retain(|p| p != &npub);
    if app_config.active_profile.as_deref() == Some(npub.as_str()) {
        app_config.active_profile = None;
        state.set_config_dir(state.base_dir.clone());
    }
    config::save_app_config(&state.base_dir, &app_config)?;
    Ok(())
}

/// List known profiles with name and picture resolved from each profile's config.json.
/// Returns a JSON array of objects: [{ "npub": "...", "name": "...", "picture": "..." }, ...]
#[tauri::command]
fn list_profiles(state: tauri::State<AppState>) -> Result<String, String> {
    let app_config = config::load_app_config(&state.base_dir)
        .unwrap_or_else(|_| config::AppConfig::new());
    let mut json = String::from("[");
    for (i, npub) in app_config.known_profiles.iter().enumerate() {
        let profile_dir = config::get_profile_dir(&state.base_dir, npub);
        let cfg = config::load_config(&profile_dir).ok();
        let name = cfg.as_ref().map(|c| c.name.as_str()).unwrap_or("Anonymous");
        let picture = cfg.as_ref().and_then(|c| c.picture.as_deref());

        if i > 0 {
            json.push(',');
        }
        json.push_str("{\"npub\":\"");
        json.push_str(npub);
        json.push_str("\",\"name\":\"");
        // Escape name for JSON safety
        for ch in name.chars() {
            match ch {
                '"' => json.push_str("\\\""),
                '\\' => json.push_str("\\\\"),
                '\n' => json.push_str("\\n"),
                '\r' => json.push_str("\\r"),
                '\t' => json.push_str("\\t"),
                _ => json.push(ch),
            }
        }
        json.push_str("\",\"picture\":");
        match picture {
            Some(url) => {
                json.push('"');
                for ch in url.chars() {
                    match ch {
                        '"' => json.push_str("\\\""),
                        '\\' => json.push_str("\\\\"),
                        _ => json.push(ch),
                    }
                }
                json.push('"');
            }
            None => json.push_str("null"),
        }
        json.push('}');
    }
    json.push(']');
    Ok(json)
}

// ============================================================
// Helper Functions
// ============================================================

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
    // Install the rustls crypto provider before any TLS connections.
    websocket::stream::install_crypto_provider();

    let base_dir: String = match config::get_config_dir() {
        Some(path) => path,
        None => {
            warn_log!("ERROR: Could not determine home directory");
            std::process::exit(1);
        }
    };

    match config::ensure_config_dir(&base_dir) {
        Ok(()) => debug_log!("Base directory ready: {}", base_dir),
        Err(e) => {
            warn_log!("ERROR: Could not create base directory: {}", e);
            std::process::exit(1);
        }
    }

    // Load app-level config to determine the active profile
    let mut app_config = match config::load_app_config(&base_dir) {
        Ok(c) => c,
        Err(e) => {
            warn_log!("Warning: Could not load plume.json: {}", e);
            config::AppConfig::new()
        }
    };

    // Migration: if known_profiles is empty but there is a legacy config.json in the base
    // directory with a public key, migrate that profile into the multi-profile structure.
    if app_config.known_profiles.is_empty() {
        if let Ok(legacy_cfg) = config::load_config(&base_dir) {
            if !legacy_cfg.public_key.is_empty() {
                if let Ok(npub) = keys::hex_to_npub(&legacy_cfg.public_key) {
                    warn_log!("[migration] Found legacy config.json with public key, migrating to profile: {}", npub);
                    if let Ok(profile_dir) = config::ensure_profile_dir(&base_dir, &npub) {
                        // Copy config to profile directory (only if one doesn't already exist there)
                        let profile_config_path = std::path::Path::new(&profile_dir).join("config.json");
                        if !profile_config_path.exists() {
                            if let Err(e) = config::save_config(&profile_dir, &legacy_cfg) {
                                warn_log!("[migration] Failed to save profile config: {}", e);
                            }
                        }
                        app_config.known_profiles.push(npub.clone());
                        app_config.active_profile = Some(npub);
                        if let Err(e) = config::save_app_config(&base_dir, &app_config) {
                            warn_log!("[migration] Failed to save app config: {}", e);
                        } else {
                            warn_log!("[migration] Migration complete");
                        }
                    }
                }
            }
        }
    }

    let config_dir = match &app_config.active_profile {
        Some(npub) => {
            let dir = config::get_profile_dir(&base_dir, npub);
            if let Err(e) = config::ensure_profile_dir(&base_dir, npub) {
                warn_log!("Warning: Could not create profile directory: {}", e);
            }
            let _ = messages_store::ensure_messages_dir(&dir);
            dir
        }
        None => base_dir.clone(),
    };

    let app_state = AppState {
        base_dir,
        active_config_dir: RwLock::new(config_dir),
    };
    
    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            get_config_dir,
            load_config,
            save_config,
            convert_public_key_to_hex,
            convert_hex_to_npub,
            convert_secret_key_to_hex,
            convert_hex_to_nsec,
            parse_key,
            decode_nostr_uri,
            fetch_notes,
            fetch_notes_from_relays,
            start_feed_stream,
            fetch_events_by_ids,
            generate_qr_svg,
            fetch_replies_to_event,
            test_relay_connection,
            get_relay_backoff_status,
            fetch_profile,
            fetch_own_profile,
            set_profile_metadata,
            verify_event,
            verify_event_id,
            verify_event_signature,
            compute_event_id,
            fetch_following,
            fetch_own_following,
            update_contact_list,
            set_contact_list,
            fetch_followers,
            fetch_own_followers,
            fetch_relay_list,
            post_note,
            post_reaction,
            post_repost,
            get_conversations,
            get_messages,
            send_dm,
            start_dm_stream,
            count_unread_dms,
            mark_dms_read,
            request_zap_invoice,
            sign_event,
            get_derived_public_key,
            generate_keypair,
            get_app_config,
            login_with_keys,
            switch_profile,
            logout,
            delete_profile,
            list_profiles,
        ])
        .setup(|app| {
            let _window = app.get_webview_window("main").unwrap();
            #[cfg(debug_assertions)]
            {
                _window.open_devtools();
            }
            warn_log!("Plume is starting...");
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("ERROR: Failed to run Tauri application");
}
