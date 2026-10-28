/*
 * relay.rs
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

//! Nostr relay message types and JSON push-parser, plus the fetch/publish
//! wrapper functions the rest of the app calls. Connection lifecycle (one
//! persistent, auto-reconnecting WebSocket per relay, demultiplexed across
//! every caller) lives in [`crate::relay_pool`] -- every function below
//! sends/receives over whatever connection that module maintains rather
//! than dialing or closing anything itself.

use bytes::BytesMut;

use crate::async_bridge::{self, mpsc};
use crate::debug_log;
use crate::json::{JsonContentHandler, JsonNumber, JsonParser};
use crate::nostr;
use crate::relay_pool;

/// Default budget for waiting on a relay's response after connecting
/// (EOSE, a fetch's results, a publish ack, …) -- Tor adds latency to
/// every round trip, so this is generous relative to what a direct
/// connection would need.
pub(crate) const RESPONSE_TIMEOUT_SECS: u32 = 20;

// ============================================================
// Relay Message Types
// ============================================================

pub enum RelayMessage {
    Event {
        _subscription_id: String,
        event: nostr::Event,
    },
    EndOfStoredEvents { _subscription_id: String },
    Notice { message: String },
    Ok { event_id: String, success: bool, message: String },
    Unknown { _raw: String },
}

pub enum StreamMessage {
    Event(nostr::Event),
    Eose,
    Notice(String),
}

// ============================================================
// Push-parser handler for relay messages
// ============================================================

/// Handler that accumulates state while parsing one relay message (top-level JSON array).
struct RelayMessageHandler {
    depth: i32,
    top_level_index: i32,
    msg_type: Option<String>,
    second_str: Option<String>,
    sub_id: Option<String>,
    ok_event_id: Option<String>,
    ok_success: bool,
    ok_message: Option<String>,
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
    result: Option<RelayMessage>,
    raw: String,
}

impl RelayMessageHandler {
    fn new(raw: String) -> Self {
        Self {
            depth: 0,
            top_level_index: -1,
            msg_type: None,
            second_str: None,
            sub_id: None,
            ok_event_id: None,
            ok_success: false,
            ok_message: None,
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
            result: None,
            raw,
        }
    }

    fn take_result(&mut self) -> Result<RelayMessage, String> {
        if let Some(r) = self.result.take() {
            return Ok(r);
        }
        match self.msg_type.as_deref() {
            Some("EOSE") => Ok(RelayMessage::EndOfStoredEvents {
                _subscription_id: self.second_str.clone().unwrap_or_default(),
            }),
            Some("NOTICE") => Ok(RelayMessage::Notice {
                message: self.second_str.clone().unwrap_or_else(|| "Unknown notice".to_string()),
            }),
            Some("OK") => Ok(RelayMessage::Ok {
                event_id: self.ok_event_id.clone().unwrap_or_default(),
                success: self.ok_success,
                message: self.ok_message.clone().unwrap_or_default(),
            }),
            _ => Ok(RelayMessage::Unknown {
                _raw: self.raw.clone(),
            }),
        }
    }
}

impl JsonContentHandler for RelayMessageHandler {
    fn start_object(&mut self) {
        self.depth += 1;
        if self.depth == 1 {
            self.top_level_index += 1;
        }
        if self.depth == 2 && self.msg_type.as_deref() == Some("EVENT") {
            self.current_field = None;
            self.event_id = None;
            self.event_pubkey = None;
            self.event_created_at = 0;
            self.event_kind = 0;
            self.event_content.clear();
            self.event_sig = None;
            self.event_tags.clear();
        }
    }

    fn end_object(&mut self) {
        self.depth -= 1;
        if self.depth == 1
            && self.msg_type.as_deref() == Some("EVENT")
            && self.second_str.is_some()
        {
            let sub_id_owned = self.sub_id.clone().unwrap_or_default();
            let ev = nostr::Event {
                id: self.event_id.clone().unwrap_or_default(),
                pubkey: self.event_pubkey.clone().unwrap_or_default(),
                created_at: self.event_created_at,
                kind: self.event_kind,
                tags: self.event_tags.clone(),
                content: self.event_content.clone(),
                sig: self.event_sig.clone().unwrap_or_default(),
            };
            self.result = Some(RelayMessage::Event {
                _subscription_id: sub_id_owned,
                event: ev,
            });
        }
    }

    fn start_array(&mut self) {
        self.depth += 1;
        if self.depth == 1 {
            self.top_level_index = 0;
        } else if self.tags_depth == 1 {
            self.tags_depth = 2;
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
        let s = value.to_string();
        if self.depth == 1 {
            self.top_level_index += 1;
            if self.top_level_index == 1 {
                self.msg_type = Some(s.clone());
            } else if self.top_level_index == 2 {
                self.second_str = Some(s.clone());
                self.sub_id = Some(s.clone());
                self.ok_event_id = Some(s);
            } else if self.top_level_index == 4 && self.msg_type.as_deref() == Some("OK") {
                self.ok_message = Some(s);
            }
        } else if self.tags_depth == 2 {
            self.current_tag.push(s);
        } else if self.depth >= 2 && self.tags_depth == 0 {
            if let Some(ref f) = self.current_field {
                match f.as_str() {
                    "id" => self.event_id = Some(s),
                    "pubkey" => self.event_pubkey = Some(s),
                    "content" => self.event_content = s,
                    "sig" => self.event_sig = Some(s),
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

    fn boolean_value(&mut self, value: bool) {
        if self.depth == 1 {
            self.top_level_index += 1;
            if self.top_level_index == 3 && self.msg_type.as_deref() == Some("OK") {
                self.ok_success = value;
            }
        }
    }

    fn null_value(&mut self) {}
}

/// Parse a single relay message using our JSON push parser.
pub fn parse_relay_message(message: &str) -> Result<RelayMessage, String> {
    let raw = message.to_string();
    let mut handler = RelayMessageHandler::new(raw);
    let mut parser = JsonParser::new();
    let mut buf = BytesMut::from(message.as_bytes());
    parser.receive(&mut buf, &mut handler).map_err(|e| format!("Relay message parse error: {}", e))?;
    parser.close(&mut handler).map_err(|e| format!("Relay message parse error: {}", e))?;
    handler.take_result()
}

// ============================================================
// One-off async relay functions (fetch, publish)
// ============================================================

/// Fetch notes from a single relay (async, with timeout). Sends the REQ
/// over that relay's shared, persistent connection (dialing/waiting for
/// one to come up first if necessary) via
/// [`relay_pool::query_streaming`] -- it does not dial or close anything
/// of its own.
pub async fn fetch_notes_from_relay(
    relay_url: &str,
    filter: &nostr::Filter,
    timeout_seconds: u32,
) -> Result<Vec<nostr::Event>, String> {
    let (tx, mut rx) = mpsc::unbounded_channel();

    let url = relay_url.to_string();
    let filter = filter.clone();
    let timeout = timeout_seconds;

    async_bridge::spawn(async move {
        relay_pool::query_streaming(url, filter, timeout, tx).await;
    });

    let mut events: Vec<nostr::Event> = Vec::new();
    while let Some(msg) = rx.recv().await {
        match msg {
            StreamMessage::Event(event) => {
                events.push(event);
            }
            StreamMessage::Eose => {
                break;
            }
            StreamMessage::Notice(msg) => {
                debug_log!("Notice from {}: {}", relay_url, msg);
            }
        }
    }

    debug_log!("Fetched {} events from {}", events.len(), relay_url);
    Ok(events)
}

/// Fetch profile metadata for a public key from a relay.
pub async fn fetch_profile_from_relay(
    relay_url: &str,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Option<nostr::ProfileMetadata>, String> {
    let filter = nostr::filter_profile_by_author(pubkey);
    let events = fetch_notes_from_relay(relay_url, &filter, timeout_seconds).await?;
    
    let mut best_event: Option<&nostr::Event> = None;
    for event in &events {
        if event.kind == nostr::KIND_METADATA {
            match &best_event {
                None => best_event = Some(event),
                Some(current) => {
                    if event.created_at > current.created_at {
                        best_event = Some(event);
                    }
                }
            }
        }
    }
    
    match best_event {
        Some(event) => {
            match nostr::parse_profile(&event.content) {
                Ok(mut profile) => {
                    profile.created_at = Some(event.created_at);
                    Ok(Some(profile))
                }
                Err(e) => Err(e),
            }
        }
        None => Ok(None),
    }
}

/// Fetch profile from multiple relays, trying all of them at once (each
/// relay's connection is independent of the others, so trying them one at
/// a time would stack every relay's response timeout on top of the last).
/// Takes the first successful result.
pub async fn fetch_profile_from_relays(
    relay_urls: &Vec<String>,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Option<nostr::ProfileMetadata>, String> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    for relay_url in relay_urls {
        let relay_url = relay_url.clone();
        let pubkey = pubkey.to_string();
        let tx = tx.clone();
        async_bridge::spawn(async move {
            let result = fetch_profile_from_relay(&relay_url, &pubkey, timeout_seconds).await;
            let _ = tx.send((relay_url, result));
        });
    }
    drop(tx);
    while let Some((relay_url, result)) = rx.recv().await {
        match result {
            Ok(Some(profile)) => return Ok(Some(profile)),
            Ok(None) => continue,
            Err(e) => {
                debug_log!("Error fetching profile from {}: {}", relay_url, e);
                continue;
            }
        }
    }
    Ok(None)
}

/// Fetch a user's contact list (who they follow) from a relay.
pub async fn fetch_following_from_relay(
    relay_url: &str,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Option<nostr::ContactList>, String> {
    let filter = nostr::filter_contact_list_by_author(pubkey);
    let events = fetch_notes_from_relay(relay_url, &filter, timeout_seconds).await?;
    
    let mut best_event: Option<&nostr::Event> = None;
    for event in &events {
        if event.kind == nostr::KIND_CONTACTS {
            match &best_event {
                None => best_event = Some(event),
                Some(current) => {
                    if event.created_at > current.created_at {
                        best_event = Some(event);
                    }
                }
            }
        }
    }
    
    match best_event {
        Some(event) => Ok(Some(nostr::parse_contact_list(event)?)),
        None => Ok(None),
    }
}

/// Fetch following from multiple relays.
pub async fn fetch_following_from_relays(
    relay_urls: &Vec<String>,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Option<nostr::ContactList>, String> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    for relay_url in relay_urls {
        let relay_url = relay_url.clone();
        let pubkey = pubkey.to_string();
        let tx = tx.clone();
        async_bridge::spawn(async move {
            let result = fetch_following_from_relay(&relay_url, &pubkey, timeout_seconds).await;
            let _ = tx.send((relay_url, result));
        });
    }
    drop(tx);
    while let Some((relay_url, result)) = rx.recv().await {
        match result {
            Ok(Some(contact_list)) => return Ok(Some(contact_list)),
            Ok(None) => continue,
            Err(e) => {
                debug_log!("Error fetching following from {}: {}", relay_url, e);
                continue;
            }
        }
    }
    Ok(None)
}

/// Fetch followers (who follows a user) from a relay.
pub async fn fetch_followers_from_relay(
    relay_url: &str,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Vec<nostr::FollowerInfo>, String> {
    let filter = nostr::filter_followers_by_pubkey(pubkey);
    let events = fetch_notes_from_relay(relay_url, &filter, timeout_seconds).await?;
    
    let mut seen_pubkeys: Vec<String> = Vec::new();
    let mut followers: Vec<nostr::FollowerInfo> = Vec::new();
    for event in &events {
        if !seen_pubkeys.contains(&event.pubkey) {
            seen_pubkeys.push(event.pubkey.clone());
            followers.push(nostr::FollowerInfo { pubkey: event.pubkey.clone() });
        }
    }
    Ok(followers)
}

/// Fetch followers from multiple relays at once and combine results (unlike
/// the "first success wins" fetches above, every relay's followers are
/// unioned together, but still queried concurrently rather than one at a
/// time).
pub async fn fetch_followers_from_relays(
    relay_urls: &Vec<String>,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Vec<nostr::FollowerInfo>, String> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    for relay_url in relay_urls {
        let relay_url = relay_url.clone();
        let pubkey = pubkey.to_string();
        let tx = tx.clone();
        async_bridge::spawn(async move {
            let result = fetch_followers_from_relay(&relay_url, &pubkey, timeout_seconds).await;
            let _ = tx.send((relay_url, result));
        });
    }
    drop(tx);
    let mut all_followers: Vec<nostr::FollowerInfo> = Vec::new();
    let mut seen_pubkeys: Vec<String> = Vec::new();
    while let Some((relay_url, result)) = rx.recv().await {
        match result {
            Ok(followers) => {
                for follower in followers {
                    if !seen_pubkeys.contains(&follower.pubkey) {
                        seen_pubkeys.push(follower.pubkey.clone());
                        all_followers.push(follower);
                    }
                }
            }
            Err(e) => {
                debug_log!("Error fetching followers from {}: {}", relay_url, e);
                continue;
            }
        }
    }
    Ok(all_followers)
}

/// Fetch a user's relay list (kind 10002) from a single relay.
pub async fn fetch_relay_list_from_relay(
    relay_url: &str,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Option<Vec<String>>, String> {
    let filter = nostr::filter_relay_list_by_author(pubkey);
    let events = fetch_notes_from_relay(relay_url, &filter, timeout_seconds).await?;
    let mut best_event: Option<&nostr::Event> = None;
    for event in &events {
        if event.kind == nostr::KIND_RELAY_LIST {
            match &best_event {
                None => best_event = Some(event),
                Some(current) => {
                    if event.created_at > current.created_at {
                        best_event = Some(event);
                    }
                }
            }
        }
    }
    match best_event {
        Some(event) => match nostr::parse_relay_list(event) {
            Ok(urls) => Ok(Some(urls)),
            Err(e) => {
                debug_log!("Failed to parse relay list: {}", e);
                Ok(None)
            }
        },
        None => Ok(None),
    }
}

/// Fetch a user's relay list from multiple relays at once.
pub async fn fetch_relay_list_from_relays(
    relay_urls: &Vec<String>,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Vec<String>, String> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    for relay_url in relay_urls {
        let relay_url = relay_url.clone();
        let pubkey = pubkey.to_string();
        let tx = tx.clone();
        async_bridge::spawn(async move {
            let result = fetch_relay_list_from_relay(&relay_url, &pubkey, timeout_seconds).await;
            let _ = tx.send((relay_url, result));
        });
    }
    drop(tx);
    while let Some((relay_url, result)) = rx.recv().await {
        match result {
            Ok(Some(urls)) if !urls.is_empty() => return Ok(urls),
            Ok(_) => continue,
            Err(e) => {
                debug_log!("Error fetching relay list from {}: {}", relay_url, e);
                continue;
            }
        }
    }
    Ok(Vec::new())
}

// ============================================================
// DM Relay List (kind 10050) fetching
// ============================================================

/// Fetch a user's DM relay list (kind 10050) from a single relay.
pub async fn fetch_dm_relay_list_from_relay(
    relay_url: &str,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Option<Vec<String>>, String> {
    let filter = nostr::filter_dm_relay_list_by_author(pubkey);
    let events = fetch_notes_from_relay(relay_url, &filter, timeout_seconds).await?;
    let mut best_event: Option<&nostr::Event> = None;
    for event in &events {
        if event.kind == nostr::KIND_DM_RELAY_LIST {
            match &best_event {
                None => best_event = Some(event),
                Some(current) => {
                    if event.created_at > current.created_at {
                        best_event = Some(event);
                    }
                }
            }
        }
    }
    match best_event {
        Some(event) => match nostr::parse_dm_relay_list(event) {
            Ok(urls) => Ok(Some(urls)),
            Err(e) => {
                debug_log!("Failed to parse DM relay list: {}", e);
                Ok(None)
            }
        },
        None => Ok(None),
    }
}

/// Fetch a user's DM relay list from multiple relays at once.
pub async fn fetch_dm_relay_list_from_relays(
    relay_urls: &[String],
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Vec<String>, String> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    for relay_url in relay_urls {
        let relay_url = relay_url.clone();
        let pubkey = pubkey.to_string();
        let tx = tx.clone();
        async_bridge::spawn(async move {
            let result = fetch_dm_relay_list_from_relay(&relay_url, &pubkey, timeout_seconds).await;
            let _ = tx.send((relay_url, result));
        });
    }
    drop(tx);
    while let Some((relay_url, result)) = rx.recv().await {
        match result {
            Ok(Some(urls)) if !urls.is_empty() => return Ok(urls),
            Ok(_) => continue,
            Err(e) => {
                debug_log!("Error fetching DM relay list from {}: {}", relay_url, e);
                continue;
            }
        }
    }
    Ok(Vec::new())
}

// ============================================================
// Event Publishing (async)
// ============================================================

pub struct PublishResult {
    pub relay_url: String,
    pub success: bool,
    pub message: String,
}

/// Publish an event to a single relay and wait for OK response (async).
pub async fn publish_event_to_relay(
    relay_url: &str,
    event: &nostr::Event,
    timeout_seconds: u32,
) -> Result<PublishResult, String> {
    relay_pool::publish(relay_url, event, timeout_seconds).await
}

/// Publish an event to multiple relays.
pub async fn publish_event_to_relays(
    relay_urls: &Vec<String>,
    event: &nostr::Event,
    timeout_seconds: u32,
) -> Vec<PublishResult> {
    let mut results: Vec<PublishResult> = Vec::new();
    
    for relay_url in relay_urls {
        match publish_event_to_relay(relay_url, event, timeout_seconds).await {
            Ok(result) => {
                debug_log!("Publish to {}: success={}, message={}", 
                         result.relay_url, result.success, result.message);
                results.push(result);
            }
            Err(e) => {
                debug_log!("Error publishing to {}: {}", relay_url, e);
                results.push(PublishResult {
                    relay_url: relay_url.to_string(),
                    success: false,
                    message: e,
                });
            }
        }
    }
    
    return results;
}

// ============================================================
// JSON helpers
// ============================================================

pub fn publish_results_to_json(results: &Vec<PublishResult>) -> String {
    let mut json = String::new();
    json.push_str("{");
    
    let success_count = results.iter().filter(|r| r.success).count();
    json.push_str("\"success_count\":");
    json.push_str(&success_count.to_string());
    
    json.push_str(",\"total_count\":");
    json.push_str(&results.len().to_string());
    
    json.push_str(",\"results\":[");
    for (i, result) in results.iter().enumerate() {
        json.push_str("{");
        json.push_str("\"relay_url\":\"");
        json.push_str(&escape_json_string(&result.relay_url));
        json.push_str("\",\"success\":");
        json.push_str(if result.success { "true" } else { "false" });
        json.push_str(",\"message\":\"");
        json.push_str(&escape_json_string(&result.message));
        json.push_str("\"}");
        if i < results.len() - 1 {
            json.push_str(",");
        }
    }
    json.push_str("]");
    
    json.push_str("}");
    return json;
}

/// Diagnostic test for the "no profiles loading" report: times
/// `fetch_profile_from_relays` against a real, already-bootstrapped Tor
/// instance and a real relay list to confirm/refute that trying relays
/// sequentially (rather than racing them) stacks each relay's timeout on
/// top of the last. The Tor-readiness-wait and "exactly one connection per
/// relay" regression tests live in `relay_pool::diag_tests` now, since
/// that's the module that actually owns connection lifecycle.
#[cfg(test)]
mod diag_tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Traces per-message arrival timing straight from a real relay, to
    /// settle whether individual EVENTs are actually delivered as they
    /// stream in (one `app.emit` per `StreamMessage::Event`, no buffering
    /// until EOSE -- which is what the Rust code appears to do on
    /// inspection) or whether something really does hold them back until
    /// EOSE. No Tauri/frontend involved -- isolates the backend half of
    /// the pipeline.
    #[test]
    fn traces_per_event_arrival_timing() {
        let addr_str = match std::env::var("PLUME_TEST_SOCKS_ADDR") {
            Ok(v) => v,
            Err(_) => {
                eprintln!("skipping: set PLUME_TEST_SOCKS_ADDR=[::1]:PORT to run this");
                return;
            }
        };
        let relay_url = std::env::var("PLUME_TEST_RELAY")
            .unwrap_or_else(|_| "wss://relay.damus.io".to_string());

        let addr: std::net::SocketAddr = addr_str.parse().expect("bad PLUME_TEST_SOCKS_ADDR");
        crate::tor::force_ready_for_test(addr);

        let (tx, mut rx) = mpsc::unbounded_channel();
        let filter = nostr::filter_recent_notes_since(200, None);
        let start = Instant::now();
        async_bridge::spawn(async move {
            relay_pool::query_streaming(relay_url, filter, RESPONSE_TIMEOUT_SECS, tx).await;
        });

        let mut event_count = 0usize;
        let mut first_event_at: Option<Duration> = None;
        let mut last_event_at: Option<Duration> = None;
        async_bridge::block_on(async {
            while let Some(msg) = rx.recv().await {
                let t = start.elapsed();
                match msg {
                    StreamMessage::Event(_) => {
                        event_count += 1;
                        if first_event_at.is_none() {
                            first_event_at = Some(t);
                        }
                        last_event_at = Some(t);
                        if event_count <= 5 || event_count % 25 == 0 {
                            eprintln!("[diag] event #{event_count} at {t:?}");
                        }
                    }
                    StreamMessage::Eose => {
                        eprintln!("[diag] EOSE at {t:?} after {event_count} events");
                        break;
                    }
                    StreamMessage::Notice(n) => eprintln!("[diag] NOTICE at {t:?}: {n}"),
                }
            }
        });

        eprintln!(
            "[diag] summary: {event_count} events, first at {first_event_at:?}, last at {last_event_at:?}"
        );
        assert!(event_count > 0, "got no events at all -- can't evaluate timing");
        if let (Some(first), Some(last)) = (first_event_at, last_event_at) {
            eprintln!(
                "[diag] spread between first and last event: {:?}",
                last.saturating_sub(first)
            );
        }
    }

    /// Regression test for "still too many open files, and the feed takes
    /// forever": fires a large burst of concurrent profile lookups (as a
    /// feed with many distinct note authors would) *at the same time* as
    /// the main feed stream, against the real configured relays, and
    /// confirms the feed stream still gets its first signal back promptly
    /// and successfully -- i.e. the background lookups queue behind the
    /// relay-connection cap instead of either starving the feed or blowing
    /// past the process's fd limit.
    #[test]
    fn feed_stream_stays_responsive_under_heavy_background_load() {
        let addr_str = match std::env::var("PLUME_TEST_SOCKS_ADDR") {
            Ok(v) => v,
            Err(_) => {
                eprintln!("skipping: set PLUME_TEST_SOCKS_ADDR=[::1]:PORT to run this");
                return;
            }
        };
        let relays_str = std::env::var("PLUME_TEST_RELAYS")
            .unwrap_or_else(|_| "wss://relay.damus.io,wss://nos.lol,wss://relay.primal.net,wss://nostr.land".to_string());
        let base_pubkey = std::env::var("PLUME_TEST_PUBKEY")
            .unwrap_or_else(|_| "4d4b2d3f5f124edf65ef73ae3f405b5984fb902a8206fd6eb0085c1a629da232".to_string());

        let addr: std::net::SocketAddr = addr_str.parse().expect("bad PLUME_TEST_SOCKS_ADDR");
        crate::tor::force_ready_for_test(addr);

        let relay_urls: Vec<String> = relays_str.split(',').map(|s| s.to_string()).collect();

        // Simulate ~60 distinct note authors each needing a profile lookup,
        // all starting at once -- the exact shape that caused this
        // regression. Mangle the pubkey per "author" so each is a genuinely
        // independent lookup, not something any cache could short-circuit.
        const SIMULATED_AUTHORS: usize = 60;
        for i in 0..SIMULATED_AUTHORS {
            let relay_urls = relay_urls.clone();
            let mut pubkey = base_pubkey.clone();
            let suffix = format!("{i:04x}");
            pubkey.replace_range(pubkey.len() - suffix.len().., &suffix);
            async_bridge::spawn(async move {
                let _ = fetch_profile_from_relays(&relay_urls, &pubkey, RESPONSE_TIMEOUT_SECS).await;
            });
        }

        // Give the burst a moment to actually start dialing before the
        // feed stream joins the race for the same limited connection slots.
        std::thread::sleep(Duration::from_millis(200));

        let (tx, mut rx) = mpsc::unbounded_channel();
        for relay_url in &relay_urls {
            let relay_url = relay_url.clone();
            let filter = nostr::filter_recent_notes_since(20, None);
            let tx = tx.clone();
            async_bridge::spawn(async move {
                relay_pool::query_streaming(relay_url, filter, RESPONSE_TIMEOUT_SECS, tx).await;
            });
        }
        drop(tx);

        let start = Instant::now();
        let first_signal = async_bridge::block_on(rx.recv());
        let elapsed = start.elapsed();
        eprintln!("[diag] first feed signal under heavy background load arrived after {elapsed:?}: {}",
            match &first_signal {
                Some(StreamMessage::Event(_)) => "Event",
                Some(StreamMessage::Eose) => "Eose",
                Some(StreamMessage::Notice(_)) => "Notice",
                None => "channel closed with nothing",
            });

        assert!(first_signal.is_some(), "feed stream produced nothing at all under background load");
        assert!(
            elapsed < Duration::from_secs(relay_pool::CONNECT_TIMEOUT_SECS + RESPONSE_TIMEOUT_SECS as u64),
            "feed stream's first signal took {elapsed:?} -- the background burst is starving it rather than just queuing behind it"
        );
    }

    #[test]
    fn fetch_profile_timing() {
        let addr_str = match std::env::var("PLUME_TEST_SOCKS_ADDR") {
            Ok(v) => v,
            Err(_) => {
                eprintln!("skipping: set PLUME_TEST_SOCKS_ADDR=[::1]:PORT to run this");
                return;
            }
        };
        let relays_str = std::env::var("PLUME_TEST_RELAYS")
            .unwrap_or_else(|_| "wss://relay.damus.io,wss://nos.lol,wss://relay.primal.net,wss://nostr.land".to_string());
        let pubkey = std::env::var("PLUME_TEST_PUBKEY")
            .unwrap_or_else(|_| "4d4b2d3f5f124edf65ef73ae3f405b5984fb902a8206fd6eb0085c1a629da232".to_string());

        let addr: std::net::SocketAddr = addr_str.parse().expect("bad PLUME_TEST_SOCKS_ADDR");
        crate::tor::force_ready_for_test(addr);

        let relay_urls: Vec<String> = relays_str.split(',').map(|s| s.to_string()).collect();

        let start = Instant::now();
        let result = async_bridge::block_on(fetch_profile_from_relays(
            &relay_urls,
            &pubkey,
            RESPONSE_TIMEOUT_SECS,
        ));
        let summary = match &result {
            Ok(Some(_)) => "found a profile".to_string(),
            Ok(None) => "no profile found on any relay".to_string(),
            Err(e) => format!("error: {e}"),
        };
        eprintln!(
            "[diag] fetch_profile_from_relays across {} relays took {:?}: {summary}",
            relay_urls.len(),
            start.elapsed(),
        );
    }

    /// Same, but under the kind of concurrent load the real app generates at
    /// startup: a feed-stream fan-out and a standing DM subscription against
    /// the same relays at the same time, in case contention for Tor
    /// circuits (not the lone-request case above) is what's actually
    /// slow/hanging.
    #[test]
    fn fetch_profile_timing_under_load() {
        let addr_str = match std::env::var("PLUME_TEST_SOCKS_ADDR") {
            Ok(v) => v,
            Err(_) => {
                eprintln!("skipping: set PLUME_TEST_SOCKS_ADDR=[::1]:PORT to run this");
                return;
            }
        };
        let relays_str = std::env::var("PLUME_TEST_RELAYS")
            .unwrap_or_else(|_| "wss://relay.damus.io,wss://nos.lol,wss://relay.primal.net,wss://nostr.land".to_string());
        let pubkey = std::env::var("PLUME_TEST_PUBKEY")
            .unwrap_or_else(|_| "4d4b2d3f5f124edf65ef73ae3f405b5984fb902a8206fd6eb0085c1a629da232".to_string());

        let addr: std::net::SocketAddr = addr_str.parse().expect("bad PLUME_TEST_SOCKS_ADDR");
        crate::tor::force_ready_for_test(addr);

        let relay_urls: Vec<String> = relays_str.split(',').map(|s| s.to_string()).collect();

        // Fire off a feed-stream-style fan-out (same shape as
        // `start_feed_stream`) and a standing DM-style subscription per
        // relay (same shape as the DM sync at startup) to occupy the same
        // relay connections the profile fetch will want -- under the
        // pool, these now share a connection with the profile fetch rather
        // than each dialing their own, which is exactly the thing this
        // test needs to still be exercising realistically.
        for relay_url in &relay_urls {
            let url = relay_url.clone();
            let filter = nostr::filter_recent_notes_since(20, None);
            async_bridge::spawn(async move {
                let (tx, mut rx) = mpsc::unbounded_channel();
                async_bridge::spawn(async move {
                    relay_pool::query_streaming(url, filter, RESPONSE_TIMEOUT_SECS, tx).await;
                });
                while rx.recv().await.is_some() {}
            });
        }
        for relay_url in &relay_urls {
            let f = nostr::filter_profile_by_author(&pubkey);
            let (tx, mut rx) = mpsc::unbounded_channel();
            let _handle = relay_pool::subscribe_persistent(relay_url, "diag-test-load-dm", vec![f], tx);
            async_bridge::spawn(async move { while rx.recv().await.is_some() {} });
        }

        let start = Instant::now();
        let result = async_bridge::block_on(fetch_profile_from_relays(
            &relay_urls,
            &pubkey,
            RESPONSE_TIMEOUT_SECS,
        ));
        let summary = match &result {
            Ok(Some(_)) => "found a profile".to_string(),
            Ok(None) => "no profile found on any relay".to_string(),
            Err(e) => format!("error: {e}"),
        };
        eprintln!(
            "[diag] under load: fetch_profile_from_relays across {} relays took {:?}: {summary}",
            relay_urls.len(),
            start.elapsed(),
        );
    }

}

fn escape_json_string(input: &str) -> String {
    let mut output = String::new();
    for c in input.chars() {
        match c {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            _ => output.push(c),
        }
    }
    return output;
}
