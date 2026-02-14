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

//! Async relay connections using our WebSocket client and JSON push parser.
//! All functions are async; no blocking code.

use bytes::BytesMut;
use tokio::sync::mpsc;
use tokio::time::Duration;

use crate::debug_log;
use crate::json::{JsonContentHandler, JsonNumber, JsonParser};
use crate::nostr;
use crate::websocket::{WebSocketClient, WebSocketHandler};

/// Connection timeout for WebSocket connect (seconds).
const CONNECT_TIMEOUT_SECS: u64 = 5;

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
// WebSocket handler for Nostr relay streams
// ============================================================

/// WebSocket handler: parses each text frame as JSON and sends StreamMessage to tx.
struct NostrRelayHandler {
    tx: mpsc::UnboundedSender<StreamMessage>,
    should_stop: bool,
    exit_on_eose: bool,
    filter_kind_dm: Option<u32>,
}

impl WebSocketHandler for NostrRelayHandler {
    fn connected(&mut self) {}

    fn text_frame(&mut self, data: &[u8]) {
        let text = match std::str::from_utf8(data) {
            Ok(t) => t,
            Err(e) => {
                println!("[relay] invalid UTF-8 in text frame ({} bytes): {}", data.len(), e);
                return;
            }
        };
        debug_log!("[relay] text frame ({} bytes): {}",
            text.len(),
            match text.char_indices().nth(120) { Some((idx, _)) => &text[..idx], None => text }
        );
        match parse_relay_message(text) {
            Ok(RelayMessage::Event { event, .. }) => {
                debug_log!("[relay] EVENT kind={} id={}", event.kind, &event.id[..8.min(event.id.len())]);
                let send = match self.filter_kind_dm {
                    Some(kind) => event.kind == kind,
                    None => true,
                };
                if send && self.tx.send(StreamMessage::Event(event)).is_err() {
                    self.should_stop = true;
                }
            }
            Ok(RelayMessage::EndOfStoredEvents { .. }) => {
                debug_log!("[relay] EOSE");
                if self.exit_on_eose {
                    self.should_stop = true;
                }
            }
            Ok(RelayMessage::Notice { message }) => {
                println!("[relay] NOTICE: {}", message);
                let _ = self.tx.send(StreamMessage::Notice(message));
            }
            Ok(_) => {
                debug_log!("[relay] other message type");
            }
            Err(e) => {
                println!("[relay] parse error: {}", e);
            }
        }
    }

    fn binary_frame(&mut self, _data: &[u8]) {}

    fn close(&mut self, _code: Option<u16>, _reason: &str) {
        self.should_stop = true;
    }

    fn ping(&mut self, _data: &[u8]) {}
    fn pong(&mut self, _data: &[u8]) {}

    fn failed(&mut self, _error: &std::io::Error) {
        self.should_stop = true;
    }

    fn should_stop(&self) -> bool {
        self.should_stop
    }
}

// ============================================================
// Async stream functions
// ============================================================

/// Run one relay's feed stream. Each text frame is parsed and turned into StreamMessage.
pub async fn run_relay_feed_stream(
    relay_url: String,
    filter: nostr::Filter,
    timeout_seconds: u32,
    tx: mpsc::UnboundedSender<StreamMessage>,
) {
    let conn = match tokio::time::timeout(
        Duration::from_secs(CONNECT_TIMEOUT_SECS),
        WebSocketClient::connect(&relay_url),
    ).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            println!("Failed to connect to {}: {}", relay_url, e);
            return;
        }
        Err(_) => {
            println!("Connection timeout to {}", relay_url);
            return;
        }
    };

    debug_log!("Connected to {}", relay_url);

    let subscription_id = format!(
        "plume_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    let filter_json = nostr::filter_to_json(&filter);
    let req_message = format!("[\"REQ\",\"{}\",{}]", subscription_id, filter_json);

    let mut conn = conn;
    debug_log!("[relay] sending REQ to {}: {}", relay_url, req_message);
    if let Err(e) = conn.send_text(req_message.as_bytes()).await {
        println!("[relay] failed to send REQ to {}: {}", relay_url, e);
        return;
    }
    debug_log!("[relay] REQ sent to {}, waiting for data (timeout {}s)...", relay_url, timeout_seconds);

    let mut handler = NostrRelayHandler {
        tx: tx.clone(),
        should_stop: false,
        exit_on_eose: true,
        filter_kind_dm: None,
    };

    let timeout_duration = Duration::from_secs(timeout_seconds as u64);
    match tokio::time::timeout(timeout_duration, conn.run(&mut handler)).await {
        Ok(Ok(())) => debug_log!("[relay] run completed normally for {}", relay_url),
        Ok(Err(e)) => println!("[relay] run error for {}: {}", relay_url, e),
        Err(_) => debug_log!("[relay] run timed out for {}", relay_url),
    }

    let _ = tx.send(StreamMessage::Eose);
}

/// Run a long-lived DM subscription (kind 4) with two filters. Does not exit on EOSE.
pub async fn run_relay_dm_stream(
    relay_url: String,
    filter_received: nostr::Filter,
    filter_sent: nostr::Filter,
    tx: mpsc::UnboundedSender<StreamMessage>,
) {
    let conn = match tokio::time::timeout(
        Duration::from_secs(CONNECT_TIMEOUT_SECS),
        WebSocketClient::connect(&relay_url),
    ).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            println!("DM stream: failed to connect to {}: {}", relay_url, e);
            return;
        }
        Err(_) => {
            println!("DM stream: connection timeout to {}", relay_url);
            return;
        }
    };

    let subscription_id = format!(
        "plume_dm_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    let f1 = nostr::filter_to_json(&filter_received);
    let f2 = nostr::filter_to_json(&filter_sent);
    let req_message = format!("[\"REQ\",\"{}\",{},{}]", subscription_id, f1, f2);

    let mut conn = conn;
    if conn.send_text(req_message.as_bytes()).await.is_err() {
        return;
    }

    let mut handler = NostrRelayHandler {
        tx,
        should_stop: false,
        exit_on_eose: false,
        filter_kind_dm: Some(nostr::KIND_DM),
    };
    let _ = conn.run(&mut handler).await;
}

// ============================================================
// One-off async relay functions (fetch, publish)
// ============================================================

/// Fetch notes from a single relay (async, with timeout).
pub async fn fetch_notes_from_relay(
    relay_url: &str,
    filter: &nostr::Filter,
    timeout_seconds: u32,
) -> Result<Vec<nostr::Event>, String> {
    let (tx, mut rx) = mpsc::unbounded_channel();

    let url = relay_url.to_string();
    let filter = filter.clone();
    let timeout = timeout_seconds;

    // Run the feed stream which collects events and sends Eose when done
    tokio::spawn(async move {
        run_relay_feed_stream(url, filter, timeout, tx).await;
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

/// Fetch profile from multiple relays (tries each until one succeeds).
pub async fn fetch_profile_from_relays(
    relay_urls: &Vec<String>,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Option<nostr::ProfileMetadata>, String> {
    for relay_url in relay_urls {
        match fetch_profile_from_relay(relay_url, pubkey, timeout_seconds).await {
            Ok(Some(profile)) => return Ok(Some(profile)),
            Ok(None) => continue,
            Err(e) => {
                println!("Error fetching profile from {}: {}", relay_url, e);
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
    for relay_url in relay_urls {
        match fetch_following_from_relay(relay_url, pubkey, timeout_seconds).await {
            Ok(Some(contact_list)) => return Ok(Some(contact_list)),
            Ok(None) => continue,
            Err(e) => {
                println!("Error fetching following from {}: {}", relay_url, e);
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

/// Fetch followers from multiple relays and combine results.
pub async fn fetch_followers_from_relays(
    relay_urls: &Vec<String>,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Vec<nostr::FollowerInfo>, String> {
    let mut all_followers: Vec<nostr::FollowerInfo> = Vec::new();
    let mut seen_pubkeys: Vec<String> = Vec::new();
    for relay_url in relay_urls {
        match fetch_followers_from_relay(relay_url, pubkey, timeout_seconds).await {
            Ok(followers) => {
                for follower in followers {
                    if !seen_pubkeys.contains(&follower.pubkey) {
                        seen_pubkeys.push(follower.pubkey.clone());
                        all_followers.push(follower);
                    }
                }
            }
            Err(e) => {
                println!("Error fetching followers from {}: {}", relay_url, e);
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
                println!("Failed to parse relay list: {}", e);
                Ok(None)
            }
        },
        None => Ok(None),
    }
}

/// Fetch a user's relay list from multiple relays.
pub async fn fetch_relay_list_from_relays(
    relay_urls: &Vec<String>,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Vec<String>, String> {
    for relay_url in relay_urls {
        match fetch_relay_list_from_relay(relay_url, pubkey, timeout_seconds).await {
            Ok(Some(urls)) if !urls.is_empty() => return Ok(urls),
            Ok(_) => continue,
            Err(e) => {
                println!("Error fetching relay list from {}: {}", relay_url, e);
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
    // Connect with timeout
    let mut conn = match tokio::time::timeout(
        Duration::from_secs(CONNECT_TIMEOUT_SECS),
        WebSocketClient::connect(relay_url),
    ).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => return Err(format!("Failed to connect to {}: {}", relay_url, e)),
        Err(_) => return Err(format!("Connection timeout to {}", relay_url)),
    };

    // Send the EVENT message
    let event_json = nostr::event_to_json(event);
    let publish_message = format!("[\"EVENT\",{}]", event_json);
    conn.send_text(publish_message.as_bytes()).await
        .map_err(|e| format!("Failed to send to {}: {}", relay_url, e))?;

    // Wait for OK response using a handler
    let (tx, mut rx) = mpsc::unbounded_channel::<RelayMessage>();

    struct PublishHandler {
        tx: mpsc::UnboundedSender<RelayMessage>,
        should_stop: bool,
    }

    impl WebSocketHandler for PublishHandler {
        fn connected(&mut self) {}
        fn text_frame(&mut self, data: &[u8]) {
            if let Ok(text) = std::str::from_utf8(data) {
                if let Ok(msg) = parse_relay_message(text) {
                    match &msg {
                        RelayMessage::Ok { .. } | RelayMessage::Notice { .. } => {
                            self.should_stop = true;
                        }
                        _ => {}
                    }
                    let _ = self.tx.send(msg);
                }
            }
        }
        fn binary_frame(&mut self, _data: &[u8]) {}
        fn close(&mut self, _code: Option<u16>, _reason: &str) { self.should_stop = true; }
        fn ping(&mut self, _data: &[u8]) {}
        fn pong(&mut self, _data: &[u8]) {}
        fn failed(&mut self, _error: &std::io::Error) { self.should_stop = true; }
        fn should_stop(&self) -> bool { self.should_stop }
    }

    let mut handler = PublishHandler { tx, should_stop: false };
    let timeout_duration = Duration::from_secs(timeout_seconds as u64);
    let _ = tokio::time::timeout(timeout_duration, conn.run(&mut handler)).await;

    // Check what we got
    while let Ok(msg) = rx.try_recv() {
        match msg {
            RelayMessage::Ok { event_id, success, message } => {
                if event_id == event.id {
                    return Ok(PublishResult {
                        relay_url: relay_url.to_string(),
                        success,
                        message,
                    });
                }
            }
            RelayMessage::Notice { message } => {
                if message.contains(&event.id) || message.to_lowercase().contains("error") {
                    return Ok(PublishResult {
                        relay_url: relay_url.to_string(),
                        success: false,
                        message,
                    });
                }
            }
            _ => {}
        }
    }

    Ok(PublishResult {
        relay_url: relay_url.to_string(),
        success: false,
        message: String::from("Timeout waiting for response"),
    })
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
                println!("Error publishing to {}: {}", relay_url, e);
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
