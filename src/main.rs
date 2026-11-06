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
mod async_bridge;
mod config;
mod crypto;
mod debug;
mod http;
mod json;
mod keyring_store;
mod keys;
mod media;
mod media_cache;
mod media_proxy;
mod messages_store;
mod net;
mod nostr;
mod relay;
mod relay_pool;
mod tls;
mod tor;
mod websocket;

// Import what we need from external crates
use tauri::Emitter;
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

/// Derive the public key matching a private key the user typed (e.g. at
/// login, before any profile/config exists) -- unlike
/// [`get_derived_public_key`], this takes the key directly rather than
/// reading one already saved to the active profile's config.
#[tauri::command(rename_all = "snake_case")]
fn derive_public_key(private_key: String) -> Result<String, String> {
    let secret_hex = keys::secret_key_to_hex(private_key.trim())?;
    let pubkey = crypto::get_public_key_from_secret(&secret_hex)?;
    let npub = keys::hex_to_npub(&pubkey).unwrap_or_default();
    Ok(format!("{{\"hex\":\"{}\",\"npub\":\"{}\"}}", pubkey, npub))
}

#[cfg(test)]
mod derive_public_key_tests {
    use super::*;

    #[test]
    fn derives_the_public_key_matching_a_freshly_generated_private_key() {
        let (secret_hex, pubkey_hex) = crypto::generate_keypair().unwrap();
        let expected_npub = keys::hex_to_npub(&pubkey_hex).unwrap();
        let nsec = keys::hex_to_nsec(&secret_hex).unwrap();

        let json = derive_public_key(nsec).unwrap();

        assert!(json.contains(&pubkey_hex), "expected hex pubkey {pubkey_hex} in {json}");
        assert!(json.contains(&expected_npub), "expected npub {expected_npub} in {json}");
    }

    #[test]
    fn rejects_an_invalid_private_key() {
        assert!(derive_public_key("not a key".to_string()).is_err());
    }
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
    let events = relay::fetch_notes_from_relay(&relay_url, &filter, relay::RESPONSE_TIMEOUT_SECS).await?;
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
        match relay::fetch_notes_from_relay(&relay_url, &filter, relay::RESPONSE_TIMEOUT_SECS).await {
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

/// Search notes by hashtag across relays.
#[tauri::command(rename_all = "snake_case")]
async fn search_hashtag(
    relay_urls: Vec<String>,
    hashtag: String,
    limit: u32,
) -> Result<String, String> {
    if relay_urls.is_empty() {
        return Err(String::from("No relays provided."));
    }
    let tag = hashtag.trim().trim_start_matches('#').to_lowercase();
    if tag.is_empty() {
        return Err(String::from("Hashtag is empty."));
    }
    let filter = nostr::filter_notes_by_hashtag(&tag, limit);

    let relay_count = relay_urls.len();
    let (mut all_events, fail_count) =
        relay::fetch_notes_from_relays(&relay_urls, &filter, relay::RESPONSE_TIMEOUT_SECS).await;

    if fail_count == relay_count {
        return Err(String::from("Could not reach any relay."));
    }

    all_events.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let mut seen_ids: Vec<String> = Vec::new();
    let mut unique: Vec<nostr::Event> = Vec::new();
    for event in all_events {
        if !seen_ids.contains(&event.id) {
            seen_ids.push(event.id.clone());
            unique.push(event);
        }
    }
    if unique.len() > limit as usize {
        unique.truncate(limit as usize);
    }
    Ok(events_to_json_array(&unique))
}

/// NIP-50 full-text search across relays. Falls back to client-side content filtering
/// for relays that don't support NIP-50.
#[tauri::command(rename_all = "snake_case")]
async fn search_notes(
    relay_urls: Vec<String>,
    query: String,
    limit: u32,
    authors: Option<Vec<String>>,
    since: Option<u64>,
    until: Option<u64>,
    kind_filter: Option<String>,
) -> Result<String, String> {
    if relay_urls.is_empty() {
        return Err(String::from("No relays provided."));
    }
    let q = query.trim().to_string();
    if q.is_empty() {
        return Err(String::from("Search query is empty."));
    }

    let author_list = authors.filter(|a| !a.is_empty());
    let filter = nostr::filter_notes_by_search(&q, limit, author_list.clone(), since, until);

    let relay_count = relay_urls.len();
    let (mut all_events, fail_count) =
        relay::fetch_notes_from_relays(&relay_urls, &filter, relay::RESPONSE_TIMEOUT_SECS).await;

    // NIP-50 is inconsistently implemented: some relays reject `search`
    // outright, others return it unfiltered, and others match loosely
    // enough to include plainly unrelated results -- there is no relay
    // response here that can be trusted as already relevant. Keep only
    // events that actually contain the query, the same check already used
    // below for the fallback pass.
    let lower_q = q.to_lowercase();
    all_events.retain(|event| event.content.to_lowercase().contains(&lower_q));

    // If NIP-50 search yielded nothing (relay may not support it, or nothing
    // relevant survived the check above), fall back to fetching recent
    // notes and filtering client-side the same way.
    if all_events.is_empty() {
        let fallback_filter = if let Some(ref auths) = author_list {
            nostr::filter_notes_by_authors_since(auths.clone(), limit * 4, since)
        } else {
            nostr::filter_recent_notes_since(limit * 4, since)
        };
        let (fallback_events, _) =
            relay::fetch_notes_from_relays(&relay_urls, &fallback_filter, relay::RESPONSE_TIMEOUT_SECS).await;
        all_events.extend(
            fallback_events
                .into_iter()
                .filter(|event| event.content.to_lowercase().contains(&lower_q)),
        );
    }

    if fail_count == relay_count && all_events.is_empty() {
        return Err(String::from("Could not reach any relay."));
    }

    // Apply kind filter (notes vs replies)
    if let Some(ref kf) = kind_filter {
        match kf.as_str() {
            "notes" => {
                all_events.retain(|e| {
                    e.kind == nostr::KIND_TEXT_NOTE
                        && !e.tags.iter().any(|t| t.len() >= 2 && t[0] == "e")
                        || e.kind == nostr::KIND_LONG_FORM
                });
            }
            "replies" => {
                all_events.retain(|e| {
                    e.kind == nostr::KIND_TEXT_NOTE
                        && e.tags.iter().any(|t| t.len() >= 2 && t[0] == "e")
                });
            }
            _ => {}
        }
    }

    all_events.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let mut seen_ids: Vec<String> = Vec::new();
    let mut unique: Vec<nostr::Event> = Vec::new();
    for event in all_events {
        if !seen_ids.contains(&event.id) {
            seen_ids.push(event.id.clone());
            unique.push(event);
        }
    }
    if unique.len() > limit as usize {
        unique.truncate(limit as usize);
    }
    Ok(events_to_json_array(&unique))
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
        async_bridge::block_on(async move {
            let (tx, mut rx) = async_bridge::mpsc::unbounded_channel();
            for relay_url in relay_urls {
                let filter = filter.clone();
                let tx = tx.clone();
                async_bridge::spawn(async move {
                    relay_pool::query_streaming(relay_url, filter, relay::RESPONSE_TIMEOUT_SECS, tx).await;
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
        match relay::fetch_notes_from_relay(relay_url, &filter, relay::RESPONSE_TIMEOUT_SECS).await {
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
        match relay::fetch_notes_from_relay(&relay_url, &filter, relay::RESPONSE_TIMEOUT_SECS).await {
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

/// Current Tor status, for the frontend to query if it missed the
/// `tor-status` event (e.g. a window opened after startup already resolved).
#[tauri::command]
fn get_tor_status() -> String {
    match tor::status() {
        tor::TorStatus::Starting => r#"{"state":"starting"}"#.to_string(),
        tor::TorStatus::Ready(addr) => format!(r#"{{"state":"ready","socks_addr":"{addr}"}}"#),
        tor::TorStatus::Failed(reason) => {
            format!(r#"{{"state":"failed","reason":"{}"}}"#, reason.replace('"', "\\\""))
        }
    }
}

#[tauri::command]
async fn test_relay_connection(relay_url: String) -> Result<String, String> {
    debug_log!("Testing connection to: {}", relay_url);
    // Explicit user test — bypass backoff, but clear it on success
    match async_bridge::timeout(
        std::time::Duration::from_secs(relay::RESPONSE_TIMEOUT_SECS as u64),
        crate::websocket::WebSocketClient::connect(&relay_url),
    ).await {
        Ok(Ok(conn)) => {
            // One-shot: this handle is never used again, so close it right
            // away rather than leaking the socket for the rest of the
            // process's life (see `relay::CloseOnReturn`'s doc comment).
            conn.close();
            relay_pool::record_relay_success(&relay_url);
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
        if let Some(remaining) = relay_pool::check_relay_backoff(url) {
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
    match relay::fetch_profile_from_relays(&relay_urls, &hex_pubkey, relay::RESPONSE_TIMEOUT_SECS).await {
        Ok(Some(profile)) => Ok(nostr::profile_to_json(&profile)),
        Ok(None) => Ok(String::from("{}")),
        Err(e) => Err(format!("Failed to fetch profile: {}", e)),
    }
}

/// Kicks off an event-driven fetch of the user's own profile across every
/// configured relay and returns immediately -- the result isn't a single
/// blocking round trip (see [`relay::fetch_profile_from_relays`] for that
/// shape) because waiting for the slowest relay to answer or time out
/// before showing anything makes the common case (a fast relay already has
/// the latest profile) needlessly slow. Instead, each relay reports in on
/// its own schedule: every time one arrives with a newer `created_at` than
/// anything seen so far, `own-profile-update` is emitted with it, so the UI
/// updates incrementally rather than waiting for the whole set. Once every
/// relay has answered (or failed/timed out), `own-profile-done` is emitted.
#[tauri::command(rename_all = "snake_case")]
fn start_own_profile_fetch(app: tauri::AppHandle, state: tauri::State<AppState>) -> Result<(), String> {
    let config_dir = state.config_dir();
    let cfg = config::load_config(&config_dir).map_err(|e| format!("Failed to load config: {}", e))?;
    if cfg.public_key.is_empty() {
        return Err(String::from("No public key configured"));
    }
    let relay_urls = config::effective_relays(&cfg);
    let pubkey = cfg.public_key.clone();
    let num_relays = relay_urls.len() as u32;

    std::thread::spawn(move || {
        async_bridge::block_on(async move {
            if num_relays == 0 {
                let _ = app.emit("own-profile-done", ());
                return;
            }

            let (tx, mut rx) = async_bridge::mpsc::unbounded_channel();
            for relay_url in relay_urls {
                let pubkey = pubkey.clone();
                let tx = tx.clone();
                async_bridge::spawn(async move {
                    let result = relay::fetch_profile_from_relay(&relay_url, &pubkey, relay::RESPONSE_TIMEOUT_SECS).await;
                    let _ = tx.send(result);
                });
            }
            drop(tx);

            let mut best_created_at: Option<u64> = None;
            let mut responses = 0u32;
            while let Some(result) = rx.recv().await {
                responses += 1;
                if let Ok(Some(profile)) = result {
                    let is_newer = match best_created_at {
                        Some(current) => profile.created_at > Some(current),
                        None => true,
                    };
                    if is_newer {
                        best_created_at = profile.created_at;
                        let _ = app.emit("own-profile-update", nostr::profile_to_json(&profile));
                    }
                }
                if responses >= num_relays {
                    let _ = app.emit("own-profile-done", ());
                    break;
                }
            }
        });
    });

    Ok(())
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
    match relay::fetch_following_from_relays(&relay_urls, &hex_pubkey, relay::RESPONSE_TIMEOUT_SECS).await {
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
    let relay_urls = config::effective_relays(&cfg);
    match relay::fetch_following_from_relays(&relay_urls, &cfg.public_key, relay::RESPONSE_TIMEOUT_SECS).await {
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

// Follow/unfollow a single pubkey from the profile page's Follow button.
//
// Local-cache-first: builds the updated contact list from `cfg.following`
// (kept in sync with every successful fetch/update already) instead of
// fetching the current list from relays first, and returns as soon as
// that's signed and saved locally -- publishing to relays happens
// afterward, in the background. The old version fetched from relays
// *then* published and awaited both round trips before ever returning,
// which is what made the Follow button feel unresponsive: the frontend
// only updates its own button state once this command resolves.
#[tauri::command]
async fn update_contact_list(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    add: bool,
    target_pubkey: String,
) -> Result<(), String> {
    let config_dir = state.config_dir();
    let mut cfg = match config::load_config(&config_dir) {
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

    // The fast path above assumes `cfg.following` is an accurate snapshot.
    // The one time it safely can't be trusted is when it's never been
    // populated at all this install (empty, not just stale) -- starting
    // from nothing there would publish a contact list containing only
    // `target_hex` and wipe out the user's real one on every relay that
    // accepts it. That one case still fetches from relays first, exactly
    // like every click used to.
    let mut pubkeys = if cfg.following.is_empty() {
        match relay::fetch_following_from_relays(&config::effective_relays(&cfg), &cfg.public_key, relay::RESPONSE_TIMEOUT_SECS).await {
            Ok(Some(contact_list)) => nostr::get_following_pubkeys(&contact_list),
            Ok(None) => Vec::new(),
            Err(e) => return Err(format!("Failed to fetch current following: {}", e)),
        }
    } else {
        cfg.following.clone()
    };
    if add {
        if !pubkeys.contains(&target_hex) {
            pubkeys.push(target_hex.clone());
        }
    } else {
        pubkeys.retain(|p| p != &target_hex);
    }
    let event = crypto::create_signed_contact_list(&pubkeys, &secret_key)?;

    cfg.following = pubkeys;
    if let Err(e) = config::save_config(&config_dir, &cfg) {
        warn_log!("Warning: failed to save contact list locally: {}", e);
    }

    let relays = config::effective_relays(&cfg);
    async_bridge::spawn(async move {
        let results = relay::publish_event_to_relays(&relays, &event, relay::RESPONSE_TIMEOUT_SECS).await;
        let success_count = results.iter().filter(|r| r.success).count();
        if success_count == 0 {
            warn_log!("Contact list update failed to publish to any relay");
            // Lets the frontend revert its optimistic update and tell the
            // user, rather than silently drifting from what relays
            // actually have.
            let payload = format!(r#"{{"pubkey":"{target_hex}","add":{add}}}"#);
            let _ = app.emit("contact-list-sync-failed", payload);
        } else {
            debug_log!("Contact list update published to {success_count}/{} relays", results.len());
        }
    });

    Ok(())
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
    let results = relay::publish_event_to_relays(&config::effective_relays(&cfg), &event, relay::RESPONSE_TIMEOUT_SECS).await;
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
    match relay::fetch_followers_from_relays(&relay_urls, &hex_pubkey, relay::RESPONSE_TIMEOUT_SECS).await {
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
    match relay::fetch_followers_from_relays(&config::effective_relays(&cfg), &cfg.public_key, relay::RESPONSE_TIMEOUT_SECS).await {
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
    match relay::fetch_relay_list_from_relays(&relay_urls, &hex_pubkey, relay::RESPONSE_TIMEOUT_SECS).await {
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
    let relay_urls = config::effective_relays(&cfg);
    let secret_key = match cfg.private_key {
        Some(key) => key,
        None => return Err(String::from("No private key configured. Add your nsec in Settings to post notes.")),
    };
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
    let results = relay::publish_event_to_relays(&relay_urls, &event, relay::RESPONSE_TIMEOUT_SECS).await;
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
    let results = relay::publish_event_to_relays(&config::effective_relays(&cfg), &event, relay::RESPONSE_TIMEOUT_SECS).await;
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
    let results = relay::publish_event_to_relays(&config::effective_relays(&cfg), &event, relay::RESPONSE_TIMEOUT_SECS).await;
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

/// Send a DM: NIP-17 (gift wrap) if recipient has kind 10050 DM relays, else NIP-04 fallback.
#[tauri::command(rename_all = "snake_case")]
async fn send_dm(state: tauri::State<'_, AppState>, recipient_pubkey: String, plaintext: String) -> Result<String, String> {
    let config_dir = state.config_dir();
    messages_store::ensure_messages_dir(&config_dir).map_err(|e| e.to_string())?;
    let cfg = config::load_config(&config_dir).map_err(|e| format!("Config: {}", e))?;
    let secret_hex = cfg.private_key.as_ref()
        .ok_or("No private key configured.")?
        .clone();
    let recipient_hex = keys::public_key_to_hex(recipient_pubkey.trim())
        .map_err(|e| format!("Invalid recipient: {}", e))?;

    // Check if recipient supports NIP-17 by looking up their kind 10050 DM relay list
    let recipient_dm_relays = relay::fetch_dm_relay_list_from_relays(&config::effective_relays(&cfg), &recipient_hex, relay::RESPONSE_TIMEOUT_SECS).await
        .unwrap_or_default();

    if !recipient_dm_relays.is_empty() {
        // NIP-17 path: create gift wraps
        let (wrap_for_recipient, wrap_for_self) =
            crypto::create_nip17_dm(&plaintext, &secret_hex, &recipient_hex)?;

        // Publish recipient's gift wrap to their DM relays
        let results_recipient = relay::publish_event_to_relays(
            &recipient_dm_relays, &wrap_for_recipient, 10,
        ).await;
        let recip_ok = results_recipient.iter().any(|r| r.success);
        if !recip_ok {
            return Err(String::from("Failed to publish NIP-17 DM to any of recipient's DM relays"));
        }

        // Publish self-copy to our own DM relays (or regular relays if none configured)
        let our_dm_relays = if cfg.dm_relays.is_empty() { config::effective_relays(&cfg) } else { cfg.dm_relays.clone() };
        let _ = relay::publish_event_to_relays(&our_dm_relays, &wrap_for_self, relay::RESPONSE_TIMEOUT_SECS).await;

        // Store raw gift wraps locally
        let raw_recip = nostr::event_to_json(&wrap_for_recipient);
        let raw_self = nostr::event_to_json(&wrap_for_self);
        let _ = messages_store::append_raw_event(&config_dir, &recipient_hex, &raw_recip);
        let _ = messages_store::append_raw_event(&config_dir, &recipient_hex, &raw_self);

        Ok(nostr::event_to_json(&wrap_for_recipient))
    } else {
        // NIP-04 fallback: existing kind 4 path
        let event = crypto::create_signed_dm(&recipient_hex, &plaintext, &secret_hex)?;
        let results = relay::publish_event_to_relays(&config::effective_relays(&cfg), &event, relay::RESPONSE_TIMEOUT_SECS).await;
        let success_count = results.iter().filter(|r| r.success).count();
        if success_count == 0 {
            return Err(String::from("Failed to publish DM to any relay"));
        }
        let raw_json = nostr::event_to_json(&event);
        messages_store::append_raw_event(&config_dir, &recipient_hex, &raw_json)
            .map_err(|e| format!("Published but failed to save locally: {}", e))?;
        Ok(nostr::event_to_json(&event))
    }
}

/// Start the DM stream: subscribes to both kind 4 (NIP-04) and kind 1059 (NIP-17 gift wraps).
/// Uses incremental sync with dm_sync_since (2-day safety margin for NIP-17 timestamp randomization).
/// Handles for the standing DM subscriptions `start_dm_stream` registers,
/// kept alive for the life of the app rather than discarded -- they're
/// never cancelled today, but stashing them here (instead of letting them
/// drop) keeps that an explicit choice rather than looking like a bug.
static DM_SUBSCRIPTIONS: std::sync::OnceLock<std::sync::Mutex<Vec<relay_pool::SubscriptionHandle>>> = std::sync::OnceLock::new();

#[tauri::command(rename_all = "snake_case")]
fn start_dm_stream(app: tauri::AppHandle, state: tauri::State<AppState>) -> Result<(), String> {
    let config_dir = state.config_dir();
    let cfg = config::load_config(&config_dir).map_err(|e| format!("Config: {}", e))?;
    let our_pubkey_hex = keys::public_key_to_hex(cfg.public_key.trim()).map_err(|e| format!("Public key: {}", e))?;
    let our_secret_hex = cfg.private_key.clone();
    if our_pubkey_hex.is_empty() {
        return Ok(());
    }

    // Incremental sync: 2-day safety margin for NIP-17 timestamp randomization
    let since = if cfg.dm_sync_since > 0 {
        Some(cfg.dm_sync_since.saturating_sub(172800))
    } else {
        None
    };

    // NIP-04 filters (kind 4)
    let filter_received = nostr::filter_dms_received(&our_pubkey_hex, 500, since);
    let filter_sent = nostr::filter_dms_sent(&our_pubkey_hex, 500, since);
    // NIP-17 filter (kind 1059 gift wraps addressed to us)
    let filter_gift_wraps = nostr::filter_gift_wraps_received(&our_pubkey_hex, 500, since);

    // Collect all relay URLs (regular + DM relays, deduplicated)
    let mut all_relays: Vec<String> = config::effective_relays(&cfg);
    for dm_relay in &cfg.dm_relays {
        if !all_relays.iter().any(|r| r == dm_relay) {
            all_relays.push(dm_relay.clone());
        }
    }

    std::thread::spawn(move || {
        async_bridge::block_on(async move {
            let num_relays = all_relays.len() as u32;
            let (tx, mut rx) = async_bridge::mpsc::unbounded_channel();
            let mut handles = DM_SUBSCRIPTIONS.get_or_init(|| std::sync::Mutex::new(Vec::new())).lock().unwrap();
            for relay_url in &all_relays {
                let filters = vec![filter_received.clone(), filter_sent.clone(), filter_gift_wraps.clone()];
                handles.push(relay_pool::subscribe_persistent(relay_url, "dm", filters, tx.clone()));
            }
            drop(handles);
            drop(tx);

            let mut eose_count = 0u32;
            let mut initial_sync = true;
            let mut max_created_at = cfg.dm_sync_since;

            while let Some(msg) = rx.recv().await {
                match msg {
                    relay::StreamMessage::Event(event) => {
                        // Track max created_at for incremental sync
                        if event.created_at > max_created_at {
                            max_created_at = event.created_at;
                        }

                        let other = if event.kind == nostr::KIND_GIFT_WRAP {
                            // NIP-17: lightweight unwrap to determine conversation partner
                            if let Some(ref secret) = our_secret_hex {
                                match crypto::unwrap_gift_wrap(&event, secret) {
                                    Ok((_seal, rumor)) => {
                                        let rumor_pubkey = rumor.pubkey.to_lowercase();
                                        let our = our_pubkey_hex.to_lowercase();
                                        if rumor_pubkey == our {
                                            // Outgoing: get "p" tag from rumor
                                            rumor.tags.iter()
                                                .find(|t| t.len() >= 2 && t[0] == "p")
                                                .map(|t| t[1].to_lowercase())
                                        } else {
                                            Some(rumor_pubkey)
                                        }
                                    }
                                    Err(e) => {
                                        debug_log!("Gift wrap unwrap failed: {}", e);
                                        None
                                    }
                                }
                            } else {
                                None
                            }
                        } else {
                            // NIP-04: use existing logic
                            nostr::other_pubkey_in_dm(&event, &our_pubkey_hex)
                        };

                        if let Some(other) = other {
                            let raw = nostr::event_to_json(&event);
                            match messages_store::append_raw_event(&config_dir, &other, &raw) {
                                Ok(true) => {
                                    if !initial_sync {
                                        let _ = app.emit("dm-received", (other.clone(), raw));
                                    }
                                }
                                Ok(false) => {}
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
                            // Persist dm_sync_since for next startup
                            if max_created_at > 0 {
                                if let Ok(mut cfg) = config::load_config(&config_dir) {
                                    if max_created_at > cfg.dm_sync_since {
                                        cfg.dm_sync_since = max_created_at;
                                        let _ = config::save_config(&config_dir, &cfg);
                                    }
                                }
                            }
                            let _ = app.emit("dm-sync-done", ());
                        }
                    }
                    _ => {}
                }
            }

            // Persist dm_sync_since on stream end
            if max_created_at > 0 {
                if let Ok(mut cfg) = config::load_config(&config_dir) {
                    if max_created_at > cfg.dm_sync_since {
                        cfg.dm_sync_since = max_created_at;
                        let _ = config::save_config(&config_dir, &cfg);
                    }
                }
            }
        });
    });
    Ok(())
}

/// Publish a new DM relay list (kind 10050). See
/// [`start_publish_and_confirm`] for the shared publish/confirm/adopt
/// mechanism and why nothing is written to the local config here directly.
///
/// Publishes to the union of the *old* dm_relays (so clients still watching
/// them see the move) and the *new* ones being saved -- publishing only to
/// the new list would mean anyone still watching the old one never finds
/// out it changed. This also fixes publishing having gone to the regular
/// relay list instead of any DM relay at all.
#[tauri::command(rename_all = "snake_case")]
fn start_save_dm_relay_list(app: tauri::AppHandle, state: tauri::State<AppState>, dm_relays: Vec<String>) -> Result<(), String> {
    let config_dir = state.config_dir();
    let cfg = config::load_config(&config_dir).map_err(|e| format!("Config: {}", e))?;
    let secret_hex = cfg.private_key.clone().ok_or_else(|| String::from("No private key configured."))?;
    let event = crypto::create_signed_dm_relay_list(&dm_relays, &secret_hex)?;

    let mut relay_urls = dm_relays;
    for r in &cfg.dm_relays {
        if !relay_urls.contains(r) {
            relay_urls.push(r.clone());
        }
    }
    if relay_urls.is_empty() {
        relay_urls = config::effective_relays(&cfg);
    }

    start_publish_and_confirm(app, "dm_relays", relay_urls, event, None);
    Ok(())
}

/// Publish a new main relay list (kind 10002) -- previously, saving the
/// main relay list only ever wrote it locally; nothing published it to the
/// network at all, so every other client (and Plume itself, after a config
/// reset) had no way to learn about it. See [`start_publish_and_confirm`]
/// for the shared mechanism and the DM relay list version above for why the
/// old list is included in the publish target, not just the new one.
#[tauri::command(rename_all = "snake_case")]
fn start_save_relay_list(app: tauri::AppHandle, state: tauri::State<AppState>, relays: Vec<String>) -> Result<(), String> {
    let config_dir = state.config_dir();
    let cfg = config::load_config(&config_dir).map_err(|e| format!("Config: {}", e))?;
    let secret_hex = cfg.private_key.clone().ok_or_else(|| String::from("No private key configured."))?;
    let event = crypto::create_signed_relay_list(&relays, &secret_hex)?;

    let mut relay_urls = relays;
    for r in &cfg.relays {
        if !relay_urls.contains(r) {
            relay_urls.push(r.clone());
        }
    }
    if relay_urls.is_empty() {
        relay_urls = config::effective_relays(&cfg);
    }

    start_publish_and_confirm(app, "relays", relay_urls, event, None);
    Ok(())
}

/// Publish a new mute list (kind 10000, NIP-51): `pubkeys`/`words`/`hashtags`
/// are the desired *flat* lists (what the UI shows, with no public/private
/// distinction) -- unlike the relay-list saves, this can't just build a
/// fresh event from them, since doing that would default everything to one
/// side and silently flip any entry the user (or another client) had
/// already published as public. Fetches the current network mute list
/// first and reconciles onto it (see [`nostr::MuteList::reconcile_pubkeys`]),
/// so an existing public entry stays public and only genuinely new entries
/// default to private. Publishes to the union of currently configured
/// relays and whatever relay already had the previous mute list, same
/// "don't strand clients still watching the old location" reasoning as
/// the relay-list saves.
#[tauri::command(rename_all = "snake_case")]
fn start_save_mute_list(
    app: tauri::AppHandle,
    state: tauri::State<AppState>,
    pubkeys: Vec<String>,
    words: Vec<String>,
    hashtags: Vec<String>,
) -> Result<(), String> {
    let config_dir = state.config_dir();
    let cfg = config::load_config(&config_dir).map_err(|e| format!("Config: {}", e))?;
    let secret_hex = cfg.private_key.clone().ok_or_else(|| String::from("No private key configured."))?;
    let pubkey = crypto::get_public_key_from_secret(&secret_hex)?;
    let relay_urls = config::effective_relays(&cfg);

    std::thread::spawn(move || {
        async_bridge::block_on(async move {
            let mut list = relay::fetch_mute_list_from_relays(&relay_urls, &pubkey, Some(secret_hex.clone()), relay::RESPONSE_TIMEOUT_SECS)
                .await
                .unwrap_or_else(|| nostr::MuteList::new(0));
            list.reconcile_pubkeys(&pubkeys);
            list.reconcile_words(&words);
            list.reconcile_hashtags(&hashtags);

            let event = match crypto::create_signed_mute_list(&list.public_tags, &list.private_tags, &secret_hex) {
                Ok(e) => e,
                Err(e) => {
                    let done = format!(
                        "{{\"context\":\"mute_list\",\"any_confirmed\":false,\"confirmed_count\":0,\"total\":0,\"adopted\":null,\"error\":\"{}\"}}",
                        config::escape_json_string(&e),
                    );
                    let _ = app.emit("publish-confirm-done", done);
                    return;
                }
            };

            publish_and_confirm_core(app, "mute_list", relay_urls, event, Some(secret_hex)).await;
        });
    });

    Ok(())
}

/// Kicks off an event-driven fetch of the user's own mute list and merges
/// it into the locally cached one -- same event-driven shape as
/// [`start_own_profile_fetch`], and the same "union with what's local"
/// reasoning as the relay-list/following adoption elsewhere: the network
/// copy is authoritative for anything it has, but a mute added in Plume
/// this session that hasn't been published yet must not be discarded just
/// because the network hasn't heard about it yet.
#[tauri::command(rename_all = "snake_case")]
fn start_own_mute_list_fetch(app: tauri::AppHandle, state: tauri::State<AppState>) -> Result<(), String> {
    let config_dir = state.config_dir();
    let cfg = config::load_config(&config_dir).map_err(|e| format!("Failed to load config: {}", e))?;
    if cfg.public_key.is_empty() {
        return Err(String::from("No public key configured"));
    }
    let relay_urls = config::effective_relays(&cfg);
    let pubkey = cfg.public_key.clone();
    let secret_hex = cfg.private_key.clone();

    std::thread::spawn(move || {
        async_bridge::block_on(async move {
            let list = relay::fetch_mute_list_from_relays(&relay_urls, &pubkey, secret_hex, relay::RESPONSE_TIMEOUT_SECS).await;
            let done = match list {
                Some(list) => format!(
                    "{{\"any_confirmed\":true,\"adopted\":{}}}",
                    nostr::mute_list_to_json(&list),
                ),
                None => String::from("{\"any_confirmed\":false,\"adopted\":null}"),
            };
            let _ = app.emit("own-mute-list-fetch-done", done);
        });
    });
    Ok(())
}

/// Fetch a user's DM relay list (kind 10050) from relays.
#[tauri::command(rename_all = "snake_case")]
async fn fetch_dm_relay_list(pubkey: String, relay_urls: Vec<String>) -> Result<String, String> {
    let hex_pubkey = keys::public_key_to_hex(&pubkey)
        .map_err(|e| format!("Invalid public key: {}", e))?;
    let urls = relay::fetch_dm_relay_list_from_relays(&relay_urls, &hex_pubkey, relay::RESPONSE_TIMEOUT_SECS).await?;
    let mut json = String::from("[");
    for (i, url) in urls.iter().enumerate() {
        if i > 0 { json.push(','); }
        json.push('"');
        json.push_str(&url.replace('\\', "\\\\").replace('"', "\\\""));
        json.push('"');
    }
    json.push(']');
    Ok(json)
}

// ============================================================
// Zap Invoice (NIP-57) -- uses push JSON parser for LNURL responses
// ============================================================

/// Handler for LNURL response JSON: extracts callback, allowsNostr, minSendable, maxSendable.
/// LNURL response: extracts callback, min/max, allowsNostr.
struct LnurlJsonHandler {
    current_field: Option<String>,
    callback: Option<String>,
    allows_nostr: bool,
    min_sendable: u64,
    max_sendable: u64,
}

impl LnurlJsonHandler {
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

impl JsonContentHandler for LnurlJsonHandler {
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

/// Zap callback response: extracts pr (bolt11 invoice).
struct ZapCallbackJsonHandler {
    current_field: Option<String>,
    pr: Option<String>,
}

impl ZapCallbackJsonHandler {
    fn new() -> Self {
        Self {
            current_field: None,
            pr: None,
        }
    }
}

impl JsonContentHandler for ZapCallbackJsonHandler {
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

/// ResponseHandler for the first LNURL discovery request. On complete,
/// chains into the zap callback request.
struct LnurlDiscoveryResponseHandler {
    app: tauri::AppHandle,
    relays: Vec<String>,
    target_pubkey: String,
    event_id: String,
    amount_msats: u64,
    secret_key: String,
    json_parser: JsonParser,
    json_handler: LnurlJsonHandler,
    success: bool,
}

impl http::ResponseHandler for LnurlDiscoveryResponseHandler {
    fn ok(&mut self, _response: &http::Response) {
        self.success = true;
    }
    fn error(&mut self, response: &http::Response) {
        self.success = false;
        let payload = format!(
            r#"{{"error":"LNURL endpoint returned {}"}}"#,
            response.code
        );
        let _ = self.app.emit("zap-invoice-failed", &payload);
    }
    fn header(&mut self, _name: &str, _value: &str) {}
    fn start_body(&mut self) {}
    fn body_chunk(&mut self, data: &[u8]) {
        if self.success {
            let mut buf = BytesMut::from(data);
            let _ = self.json_parser.receive(&mut buf, &mut self.json_handler);
        }
    }
    fn end_body(&mut self) {
        let _ = self.json_parser.close(&mut self.json_handler);
    }
    fn complete(&mut self) {
        if !self.success {
            return;
        }

        let callback = match self.json_handler.callback.take() {
            Some(c) => c,
            None => {
                let _ = self.app.emit(
                    "zap-invoice-failed",
                    &r#"{"error":"LNURL response missing callback"}"#,
                );
                return;
            }
        };
        if !self.json_handler.allows_nostr {
            let _ = self.app.emit(
                "zap-invoice-failed",
                &r#"{"error":"Recipient does not support Nostr zaps"}"#,
            );
            return;
        }

        let amount_msats = self
            .amount_msats
            .clamp(self.json_handler.min_sendable, self.json_handler.max_sendable);

        let event_id_opt = if self.event_id.trim().is_empty() {
            None
        } else {
            Some(self.event_id.as_str())
        };
        let zap_event = match crypto::create_signed_zap_request(
            &self.relays,
            &self.target_pubkey,
            event_id_opt,
            amount_msats,
            "",
            &self.secret_key,
        ) {
            Ok(e) => e,
            Err(e) => {
                let payload = format!(r#"{{"error":"{}"}}"#, e.replace('"', "\\\""));
                let _ = self.app.emit("zap-invoice-failed", &payload);
                return;
            }
        };
        let zap_json = nostr::event_to_json(&zap_event);
        let zap_b64 = BASE64.encode(zap_json.as_bytes());

        let sep = if callback.contains('?') { '&' } else { '?' };
        let callback_with_params = format!(
            "{}{}amount={}&nostr={}",
            callback,
            sep,
            amount_msats,
            urlencoding::encode(&zap_b64)
        );

        let parsed = match url::Url::parse(&callback_with_params) {
            Ok(u) => u,
            Err(e) => {
                let payload = format!(r#"{{"error":"Invalid callback URL: {}"}}"#, e);
                let _ = self.app.emit("zap-invoice-failed", &payload);
                return;
            }
        };
        let host = match parsed.host_str() {
            Some(h) => h.to_string(),
            None => return,
        };
        let secure = parsed.scheme() == "https";
        let port = parsed.port().unwrap_or(if secure { 443 } else { 80 });
        let path_and_query = match parsed.query() {
            Some(q) => format!("{}?{}", parsed.path(), q),
            None => parsed.path().to_string(),
        };

        let client = http::HttpClient::new(&host, port, secure);
        let handler = Box::new(ZapCallbackResponseHandler {
            app: self.app.clone(),
            json_parser: JsonParser::new(),
            json_handler: ZapCallbackJsonHandler::new(),
            success: false,
        });
        client.get(&path_and_query).send(handler);
    }
    fn failed(&mut self, error: &std::io::Error) {
        let payload = format!(
            r#"{{"error":"{}"}}"#,
            error.to_string().replace('"', "\\\"")
        );
        let _ = self.app.emit("zap-invoice-failed", &payload);
    }
}

/// ResponseHandler for the zap callback request. Extracts invoice, emits event.
struct ZapCallbackResponseHandler {
    app: tauri::AppHandle,
    json_parser: JsonParser,
    json_handler: ZapCallbackJsonHandler,
    success: bool,
}

impl http::ResponseHandler for ZapCallbackResponseHandler {
    fn ok(&mut self, _response: &http::Response) {
        self.success = true;
    }
    fn error(&mut self, response: &http::Response) {
        self.success = false;
        let payload = format!(
            r#"{{"error":"Zap callback returned {}"}}"#,
            response.code
        );
        let _ = self.app.emit("zap-invoice-failed", &payload);
    }
    fn header(&mut self, _name: &str, _value: &str) {}
    fn start_body(&mut self) {}
    fn body_chunk(&mut self, data: &[u8]) {
        if self.success {
            let mut buf = BytesMut::from(data);
            let _ = self.json_parser.receive(&mut buf, &mut self.json_handler);
        }
    }
    fn end_body(&mut self) {
        let _ = self.json_parser.close(&mut self.json_handler);
    }
    fn complete(&mut self) {
        if !self.success {
            return;
        }
        if let Some(pr) = self.json_handler.pr.take() {
            let pr_escaped = pr.replace('\\', "\\\\").replace('"', "\\\"");
            let payload = format!(r#"{{"pr":"{}"}}"#, pr_escaped);
            let _ = self.app.emit("zap-invoice-ready", &payload);
        } else {
            let _ = self.app.emit(
                "zap-invoice-failed",
                &r#"{"error":"Callback response missing pr (invoice)"}"#,
            );
        }
    }
    fn failed(&mut self, error: &std::io::Error) {
        let payload = format!(
            r#"{{"error":"{}"}}"#,
            error.to_string().replace('"', "\\\"")
        );
        let _ = self.app.emit("zap-invoice-failed", &payload);
    }
}

/// Fire-and-forget: starts zap invoice request. Invoice delivered via
/// "zap-invoice-ready" event, errors via "zap-invoice-failed" event.
#[tauri::command]
async fn request_zap_invoice(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    target_lud16: String,
    amount_sats: u32,
    event_id: String,
    target_pubkey: String,
) -> Result<(), String> {
    let config_dir = state.config_dir();
    let cfg = config::load_config(&config_dir).map_err(|e| format!("Failed to load config: {}", e))?;
    let secret_key = cfg
        .private_key
        .as_ref()
        .ok_or("No private key configured.")?
        .clone();
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

    let client = http::HttpClient::new(domain, 443, true);
    let path = format!("/.well-known/lnurlp/{}", lnurl_user);
    let handler = Box::new(LnurlDiscoveryResponseHandler {
        app,
        relays: config::effective_relays(&cfg),
        target_pubkey,
        event_id,
        amount_msats,
        secret_key,
        json_parser: JsonParser::new(),
        json_handler: LnurlJsonHandler::new(),
        success: false,
    });
    client.get(&path).send(handler);

    Ok(())
}

// ============================================================
// Profile Metadata
// ============================================================

/// Shared publish-then-confirm-and-adopt core for every replaceable-event
/// save (profile metadata/kind 0, main relay list/kind 10002, DM relay
/// list/kind 10050). A bare relay `OK` only means "accepted," never
/// "durably stored and retrievable," and the thing a client should treat
/// as current is whatever the network actually has, not what it merely
/// intended to publish -- so this publishes `event` to every relay in
/// `relay_urls`, re-queries each one for its own resulting latest event,
/// and only then reports what to adopt locally. `publish-confirm-progress`
/// is emitted as each relay answers (never waiting for the slowest one
/// before any of them are visible), and one `publish-confirm-done` once
/// they've all reported, carrying whatever was actually confirmed. Both
/// events carry `context` so the frontend can drive one shared progress UI
/// for all three saves and still dispatch the adopted payload correctly.
fn start_publish_and_confirm(
    app: tauri::AppHandle,
    context: &'static str,
    relay_urls: Vec<String>,
    event: nostr::Event,
    secret_key_hex: Option<String>,
) {
    std::thread::spawn(move || {
        async_bridge::block_on(publish_and_confirm_core(app, context, relay_urls, event, secret_key_hex));
    });
}

/// The async core of [`start_publish_and_confirm`], split out so a command
/// that needs an async step of its own first (the mute list save needs to
/// fetch the current network state before it can even build the event to
/// publish) can run that, then drive this directly inside its own
/// thread/block_on, instead of nesting a second one.
async fn publish_and_confirm_core(
    app: tauri::AppHandle,
    context: &'static str,
    relay_urls: Vec<String>,
    event: nostr::Event,
    secret_key_hex: Option<String>,
) {
    let total = relay_urls.len();
    let (tx, mut rx) = async_bridge::mpsc::unbounded_channel();
    async_bridge::spawn({
        let relay_urls = relay_urls.clone();
        let event = event.clone();
        async move {
            relay::publish_and_confirm(&relay_urls, &event, relay::RESPONSE_TIMEOUT_SECS, tx).await;
        }
    });

    let mut confirmed_count = 0usize;
    let mut best_event: Option<nostr::Event> = None;
    while let Some(outcome) = rx.recv().await {
        if outcome.confirmed {
            confirmed_count += 1;
        }
        if let Some(e) = &outcome.latest_event {
            let is_newer = best_event.as_ref().map(|b| e.created_at > b.created_at).unwrap_or(true);
            if is_newer {
                best_event = Some(e.clone());
            }
        }
        let progress = format!(
            "{{\"context\":\"{}\",\"relay_url\":\"{}\",\"accepted\":{},\"confirmed\":{}}}",
            context,
            config::escape_json_string(&outcome.relay_url),
            outcome.accepted,
            outcome.confirmed,
        );
        let _ = app.emit("publish-confirm-progress", progress);
    }

    let adopted_json = if confirmed_count > 0 {
        best_event.as_ref().and_then(|e| match context {
            "profile" => {
                let mut p = nostr::parse_profile(&e.content).ok()?;
                p.created_at = Some(e.created_at);
                Some(nostr::profile_to_json(&p))
            }
            "relays" => Some(string_array_to_json(&nostr::parse_relay_list(e).ok()?)),
            "dm_relays" => Some(string_array_to_json(&nostr::parse_dm_relay_list(e).ok()?)),
            "mute_list" => {
                let mut list = nostr::parse_mute_list_public_tags(e).ok()?;
                if let Some(secret_hex) = &secret_key_hex {
                    if !e.content.trim().is_empty() {
                        if let Ok(plaintext) = crypto::decrypt_mute_list_content(&e.content, secret_hex) {
                            list.private_tags = nostr::parse_mute_list_tags_json(&plaintext).unwrap_or_default();
                        }
                    }
                }
                Some(nostr::mute_list_to_json(&list))
            }
            _ => None,
        })
    } else {
        None
    };

    let done = format!(
        "{{\"context\":\"{}\",\"any_confirmed\":{},\"confirmed_count\":{},\"total\":{},\"adopted\":{}}}",
        context,
        confirmed_count > 0,
        confirmed_count,
        total,
        adopted_json.as_deref().unwrap_or("null"),
    );
    let _ = app.emit("publish-confirm-done", done);
}

fn string_array_to_json(items: &[String]) -> String {
    let mut json = String::from("[");
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push('"');
        json.push_str(&config::escape_json_string(item));
        json.push('"');
    }
    json.push(']');
    json
}

#[tauri::command(rename_all = "snake_case")]
fn start_save_profile(app: tauri::AppHandle, state: tauri::State<AppState>, profile_json: String) -> Result<(), String> {
    let config_dir = state.config_dir();
    let cfg = config::load_config(&config_dir).map_err(|e| format!("Failed to load config: {}", e))?;
    let secret_key = cfg.private_key.clone().ok_or_else(|| String::from("No private key configured."))?;
    let profile = nostr::parse_profile(&profile_json).map_err(|e| format!("Invalid profile JSON: {}", e))?;
    let content = nostr::profile_to_content(&profile);
    let event = crypto::create_signed_metadata_event(&content, &secret_key)
        .map_err(|e| format!("Failed to create profile event: {}", e))?;
    let relay_urls = config::effective_relays(&cfg);
    start_publish_and_confirm(app, "profile", relay_urls, event, None);
    Ok(())
}

/// Fetch zap receipts (kind 9735) for a given pubkey (for profile zaps tab).
#[tauri::command(rename_all = "snake_case")]
async fn fetch_zap_receipts(pubkey: String, relay_urls: Vec<String>, limit: Option<u32>) -> Result<String, String> {
    let hex_pubkey = keys::public_key_to_hex(&pubkey)
        .map_err(|e| format!("Invalid public key: {}", e))?;
    let filter = nostr::filter_zap_receipts_by_pubkey(&hex_pubkey, limit.unwrap_or(50));
    let mut all_events: Vec<nostr::Event> = Vec::new();
    for relay_url in &relay_urls {
        match relay::fetch_notes_from_relay(relay_url, &filter, relay::RESPONSE_TIMEOUT_SECS).await {
            Ok(events) => all_events.extend(events),
            Err(_) => continue,
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut receipts: Vec<nostr::ZapReceiptInfo> = Vec::new();
    all_events.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    for event in &all_events {
        if seen.insert(event.id.to_lowercase()) {
            if let Some(info) = nostr::parse_zap_receipt(event) {
                receipts.push(info);
            }
        }
    }
    Ok(nostr::zap_receipts_to_json(&receipts))
}

/// Fetch zap totals (sats) per event ID for displaying amounts on note cards.
#[tauri::command(rename_all = "snake_case")]
async fn fetch_note_zap_totals(event_ids: Vec<String>, relay_urls: Vec<String>) -> Result<String, String> {
    if event_ids.is_empty() {
        return Ok(String::from("{}"));
    }
    let filter = nostr::filter_zap_receipts_by_events(event_ids, 500);
    let mut all_events: Vec<nostr::Event> = Vec::new();
    for relay_url in &relay_urls {
        match relay::fetch_notes_from_relay(relay_url, &filter, relay::RESPONSE_TIMEOUT_SECS).await {
            Ok(events) => all_events.extend(events),
            Err(_) => continue,
        }
    }
    let mut seen = std::collections::HashSet::new();
    let mut receipts: Vec<nostr::ZapReceiptInfo> = Vec::new();
    for event in &all_events {
        if seen.insert(event.id.to_lowercase()) {
            if let Some(info) = nostr::parse_zap_receipt(event) {
                receipts.push(info);
            }
        }
    }
    Ok(nostr::zap_totals_to_json(&receipts))
}

// ============================================================
// Media Upload Commands
// ============================================================

/// Open a native file dialog to pick a media file for upload.
/// Returns a JSON string `{"path":"...","name":"..."}` if a file was selected,
/// or an empty string if cancelled.
#[tauri::command(rename_all = "snake_case")]
async fn pick_media_file() -> Result<String, String> {
    let handle = rfd::AsyncFileDialog::new()
        .add_filter(
            "Media",
            &[
                "png", "jpg", "jpeg", "gif", "webp", "svg", "mp4", "webm", "mov", "avi", "mp3",
                "ogg", "wav", "flac", "pdf",
            ],
        )
        .set_title("Select file to upload")
        .pick_file()
        .await;

    match handle {
        Some(file) => {
            let path = file.path().to_string_lossy().to_string();
            let name = file.file_name();
            Ok(format!(
                r#"{{"path":"{}","name":"{}"}}"#,
                path.replace('\\', "\\\\").replace('"', "\\\""),
                name.replace('\\', "\\\\").replace('"', "\\\""),
            ))
        }
        None => Ok(String::new()),
    }
}

/// Upload a file to the configured media server.
/// Accepts a filesystem path from the frontend (file is read by the backend).
/// Fire-and-forget: starts upload, returns immediately. Progress, completion,
/// and failure delivered via "upload-progress", "upload-complete",
/// "upload-failed" Tauri events.
#[tauri::command(rename_all = "snake_case")]
async fn upload_media(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    file_path: String,
    file_name: String,
    upload_id: String,
) -> Result<(), String> {
    let config_dir = state.config_dir();
    let cfg = config::load_config(&config_dir).map_err(|e| format!("Config: {}", e))?;
    let secret_key = cfg
        .private_key
        .as_ref()
        .ok_or("No private key configured. Add your nsec in Settings to upload.")?
        .clone();
    let server_url = cfg.media_server_url.clone();
    if server_url.is_empty() {
        return Err(String::from(
            "No media server configured. Set one in Settings.",
        ));
    }

    let content_type = mime_guess::from_path(&file_name)
        .first_or_octet_stream()
        .to_string();

    media::start_upload(
        app,
        &server_url,
        &file_path,
        &file_name,
        &content_type,
        &secret_key,
        &upload_id,
    );

    Ok(())
}

/// Fire-and-forget: starts delete, returns immediately.
#[tauri::command(rename_all = "snake_case")]
async fn delete_media(
    state: tauri::State<'_, AppState>,
    file_hash: String,
) -> Result<(), String> {
    let config_dir = state.config_dir();
    let cfg = config::load_config(&config_dir).map_err(|e| format!("Config: {}", e))?;
    let secret_key = cfg
        .private_key
        .as_ref()
        .ok_or("No private key configured.")?
        .clone();
    let server_url = cfg.media_server_url.clone();
    if server_url.is_empty() {
        return Err(String::from("No media server configured."));
    }

    media::start_delete(&server_url, &file_hash, &secret_key);
    Ok(())
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

#[tauri::command(rename_all = "snake_case")]
fn is_keyring_available() -> bool {
    keyring_store::is_available()
}

/// Returns the active profile's private key as an nsec string, for the
/// one-shot "copy to clipboard" action -- resolves transparently from
/// plaintext config or the OS keyring, same as every signing command, so
/// the frontend never needs to hold the raw secret in `state.config` just
/// to perform this conversion itself.
#[tauri::command(rename_all = "snake_case")]
fn get_nsec_string(state: tauri::State<AppState>) -> Result<String, String> {
    let cfg = config::load_config(&state.config_dir())
        .map_err(|e| format!("Failed to load config: {}", e))?;
    let secret_key = cfg.private_key.ok_or_else(|| String::from("No private key configured"))?;
    keys::hex_to_nsec(&secret_key)
}

/// Moves the active profile's private key between plaintext config and the
/// OS keyring. Shared by the first-login "store in keyring?" prompt and the
/// Settings → Keys panel's move control. Returns the updated config JSON.
#[tauri::command(rename_all = "snake_case")]
fn set_key_storage_mode(state: tauri::State<AppState>, mode: String) -> Result<String, String> {
    let target = config::KeyStorage::from_str(&mode);
    let config_dir = state.config_dir();
    let mut cfg = config::load_config(&config_dir)
        .map_err(|e| format!("Failed to load config: {}", e))?;

    if cfg.key_storage == target {
        return Ok(config::config_to_json(&cfg));
    }

    match target {
        config::KeyStorage::Keyring => {
            if !keyring_store::is_available() {
                return Err(String::from("No system keyring is available on this device."));
            }
            let secret_key = cfg.private_key.clone().ok_or_else(|| String::from("No private key configured"))?;
            keyring_store::store(&cfg.public_key, &secret_key)?;
            cfg.key_storage = config::KeyStorage::Keyring;
            config::save_config(&config_dir, &cfg)?;
        }
        config::KeyStorage::Plaintext => {
            // cfg.private_key is already resolved in memory (load_config's
            // keyring fallback), so just flip the mode and persist it.
            cfg.key_storage = config::KeyStorage::Plaintext;
            config::save_config(&config_dir, &cfg)?;
            let _ = keyring_store::delete(&cfg.public_key);
        }
    }

    Ok(config::config_to_json(&cfg))
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
        // Best-effort: if this profile's key lived in the OS keyring,
        // remove it too, so deleting a profile doesn't leave an orphaned
        // secret behind. Must happen before the directory goes away.
        if let Ok(cfg) = config::load_config(&profile_dir) {
            if cfg.key_storage == config::KeyStorage::Keyring {
                let _ = keyring_store::delete(&cfg.public_key);
            }
        }
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

/// Raise the process's soft open-file limit to its hard limit (or, on
/// macOS, to `OPEN_MAX` when the hard limit reports as infinite) before
/// anything that opens a socket runs. A GUI app launched via launchd on
/// macOS defaults to a soft `RLIMIT_NOFILE` of just 256 -- comfortably
/// exhausted by this app's own concurrent Tor dials alone (relay WebSocket
/// connections, image/avatar HTTP fetches, media uploads) once stacked on
/// top of whatever fds Tauri/WebKit already hold at baseline, well before
/// any of this app's own internal concurrency caps
/// (`relay::MAX_CONCURRENT_RELAY_CONNECTIONS`, `media_proxy::MAX_CONCURRENT_FETCHES`)
/// come anywhere near their limits. This is the same fix Chromium and other
/// desktop apps apply on launch; best-effort only -- a failure here is
/// logged and otherwise ignored rather than treated as fatal, since the app
/// can still run (just with more "Too many open files" risk) at whatever
/// limit the OS handed it.
#[cfg(unix)]
fn raise_fd_limit() {
    use std::mem::MaybeUninit;
    unsafe {
        let mut limit = MaybeUninit::<libc::rlimit>::uninit();
        if libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) != 0 {
            warn_log!("[startup] getrlimit(RLIMIT_NOFILE) failed: {}", std::io::Error::last_os_error());
            return;
        }
        let mut limit = limit.assume_init();

        #[cfg(target_os = "macos")]
        let hard = if limit.rlim_max == libc::RLIM_INFINITY {
            // setrlimit(..., RLIM_INFINITY) is rejected outright on Darwin,
            // and libc doesn't expose a usable OPEN_MAX constant for it (it
            // varies by kernel config, via `sysctl kern.maxfilesperproc`).
            // 10240 is the same fallback ceiling Chromium uses for exactly
            // this case -- comfortably above anything this app needs, well
            // under what the kernel will actually grant.
            10240
        } else {
            limit.rlim_max
        };
        #[cfg(not(target_os = "macos"))]
        let hard = limit.rlim_max;

        if limit.rlim_cur >= hard {
            return;
        }
        limit.rlim_cur = hard;
        if libc::setrlimit(libc::RLIMIT_NOFILE, &limit) != 0 {
            warn_log!("[startup] setrlimit(RLIMIT_NOFILE, {}) failed: {}", hard, std::io::Error::last_os_error());
        } else {
            debug_log!("[startup] raised RLIMIT_NOFILE soft limit to {}", hard);
        }
    }
}

#[cfg(not(unix))]
fn raise_fd_limit() {}

fn main() {
    raise_fd_limit();

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

    media_cache::init(&base_dir);

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

    let base_dir_for_tor = base_dir.clone();
    let app_state = AppState {
        base_dir,
        active_config_dir: RwLock::new(config_dir),
    };

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(app_state)
        .register_asynchronous_uri_scheme_protocol("torimg", |_ctx, request, responder| {
            debug_log!("[torimg] raw request uri: {:?}", request.uri());
            media_proxy::handle(request.uri().path(), responder);
        })
        .invoke_handler(tauri::generate_handler![
            get_config_dir,
            load_config,
            save_config,
            convert_public_key_to_hex,
            convert_hex_to_npub,
            convert_secret_key_to_hex,
            convert_hex_to_nsec,
            derive_public_key,
            parse_key,
            decode_nostr_uri,
            fetch_notes,
            fetch_notes_from_relays,
            search_notes,
            search_hashtag,
            start_feed_stream,
            fetch_events_by_ids,
            generate_qr_svg,
            fetch_replies_to_event,
            test_relay_connection,
            get_relay_backoff_status,
            get_tor_status,
            fetch_profile,
            start_own_profile_fetch,
            start_save_profile,
            start_save_relay_list,
            start_save_mute_list,
            start_own_mute_list_fetch,
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
            start_save_dm_relay_list,
            fetch_dm_relay_list,
            request_zap_invoice,
            fetch_zap_receipts,
            fetch_note_zap_totals,
            pick_media_file,
            upload_media,
            delete_media,
            sign_event,
            is_keyring_available,
            get_nsec_string,
            set_key_storage_mode,
            get_derived_public_key,
            generate_keypair,
            get_app_config,
            login_with_keys,
            switch_profile,
            logout,
            delete_profile,
            list_profiles,
        ])
        .setup(move |app| {
            warn_log!("Plume is starting...");
            tor::start(app.handle().clone(), &base_dir_for_tor);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("ERROR: Failed to build Tauri application");

    app.run(|_app_handle, event| {
        if let tauri::RunEvent::Exit = event {
            relay_pool::shutdown();
            tor::shutdown();
        }
    });
}
