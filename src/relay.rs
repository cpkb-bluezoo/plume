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

use std::net::TcpStream;
use tungstenite::{connect, stream::MaybeTlsStream, Message, WebSocket};
use url::Url;

use crate::nostr;

// --- Async stream (tokio-tungstenite + Actson) ---

use actson::feeder::SliceJsonFeeder;
use actson::{JsonEvent, JsonParser};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{connect_async, tungstenite::Message as WsMessage};

// A connection to a single Nostr relay
pub struct RelayConnection {
    // The relay URL (e.g., "wss://relay.damus.io")
    pub url: String,
    
    // The WebSocket connection (None if not connected)
    socket: Option<WebSocket<MaybeTlsStream<TcpStream>>>,
    
    // Whether we're currently connected
    pub connected: bool,
}

impl RelayConnection {
    // Create a new relay connection (not yet connected)
    pub fn new(url: &str) -> RelayConnection {
        RelayConnection {
            url: url.to_string(),
            socket: None,
            connected: false,
        }
    }
    
    // Connect to the relay
    pub fn connect(&mut self) -> Result<(), String> {
        println!("Connecting to relay: {}", self.url);
        
        // Parse the URL
        let parsed_url = match Url::parse(&self.url) {
            Ok(u) => u,
            Err(e) => return Err(format!("Invalid relay URL: {}", e)),
        };
        
        // Establish the WebSocket connection
        match connect(&parsed_url) {
            Ok((socket, response)) => {
                println!("Connected to {}! Status: {}", self.url, response.status());
                self.socket = Some(socket);
                self.connected = true;
                return Ok(());
            }
            Err(e) => {
                self.connected = false;
                return Err(format!("Failed to connect to {}: {}", self.url, e));
            }
        }
    }
    
    // Disconnect from the relay
    pub fn disconnect(&mut self) {
        if let Some(ref mut socket) = self.socket {
            // Send a close frame
            let _ = socket.close(None);
        }
        self.socket = None;
        self.connected = false;
        println!("Disconnected from: {}", self.url);
    }
    
    // Send a raw message to the relay
    pub fn send(&mut self, message: &str) -> Result<(), String> {
        if !self.connected {
            return Err(String::from("Not connected to relay"));
        }
        
        match &mut self.socket {
            Some(socket) => {
                match socket.send(Message::Text(message.to_string())) {
                    Ok(()) => {
                        println!("Sent to {}: {}", self.url, message);
                        return Ok(());
                    }
                    Err(e) => {
                        return Err(format!("Failed to send message: {}", e));
                    }
                }
            }
            None => {
                return Err(String::from("No socket connection"));
            }
        }
    }
    
    // Receive a message from the relay (blocking)
    pub fn receive(&mut self) -> Result<String, String> {
        if !self.connected {
            return Err(String::from("Not connected to relay"));
        }
        
        match &mut self.socket {
            Some(socket) => {
                match socket.read() {
                    Ok(message) => {
                        match message {
                            Message::Text(text) => {
                                return Ok(text);
                            }
                            Message::Binary(data) => {
                                // Convert binary to string
                                match String::from_utf8(data) {
                                    Ok(text) => return Ok(text),
                                    Err(e) => return Err(format!("Invalid UTF-8: {}", e)),
                                }
                            }
                            Message::Ping(_) => {
                                // Tungstenite handles pong automatically
                                return Err(String::from("Received ping"));
                            }
                            Message::Pong(_) => {
                                return Err(String::from("Received pong"));
                            }
                            Message::Close(_) => {
                                self.connected = false;
                                return Err(String::from("Connection closed by relay"));
                            }
                            Message::Frame(_) => {
                                return Err(String::from("Received raw frame"));
                            }
                        }
                    }
                    Err(e) => {
                        self.connected = false;
                        return Err(format!("Failed to receive message: {}", e));
                    }
                }
            }
            None => {
                return Err(String::from("No socket connection"));
            }
        }
    }
    
    // Subscribe to events matching a filter
    // subscription_id is a unique string to identify this subscription
    pub fn subscribe(&mut self, subscription_id: &str, filter: &nostr::Filter) -> Result<(), String> {
        // Build the REQ message: ["REQ", subscription_id, filter]
        let filter_json = nostr::filter_to_json(filter);
        let req_message = format!("[\"REQ\",\"{}\",{}]", subscription_id, filter_json);
        
        return self.send(&req_message);
    }
    
    // Close a subscription
    pub fn close_subscription(&mut self, subscription_id: &str) -> Result<(), String> {
        // Build the CLOSE message: ["CLOSE", subscription_id]
        let close_message = format!("[\"CLOSE\",\"{}\"]", subscription_id);
        
        return self.send(&close_message);
    }
    
    // Publish an event to the relay
    pub fn publish_event(&mut self, event: &nostr::Event) -> Result<(), String> {
        // Build the EVENT message: ["EVENT", event_object]
        let event_json = nostr::event_to_json(event);
        let publish_message = format!("[\"EVENT\",{}]", event_json);
        
        return self.send(&publish_message);
    }
}

// Parse a relay message
// Nostr relay messages are JSON arrays like:
//   ["EVENT", subscription_id, event]
//   ["EOSE", subscription_id]  (End Of Stored Events)
//   ["NOTICE", message]
//   ["OK", event_id, success, message]
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

// Parse a message received from a relay
pub fn parse_relay_message(message: &str) -> Result<RelayMessage, String> {
    let parsed = match json::parse(message) {
        Ok(value) => value,
        Err(e) => return Err(format!("Invalid JSON from relay: {}", e)),
    };
    
    // Check if it's an array
    if !parsed.is_array() {
        return Err(String::from("Relay message is not an array"));
    }
    
    // Get the message type (first element)
    let msg_type: &str;
    if parsed[0].is_string() {
        msg_type = parsed[0].as_str().unwrap();
    } else {
        return Err(String::from("Message type is not a string"));
    }
    
    match msg_type {
        "EVENT" => {
            // ["EVENT", subscription_id, event_object]
            let subscription_id: String;
            if parsed[1].is_string() {
                subscription_id = parsed[1].as_str().unwrap().to_string();
            } else {
                return Err(String::from("Missing subscription_id in EVENT"));
            }
            
            // The event is the third element - convert back to JSON string
            let event_json = parsed[2].dump();
            let event = nostr::parse_event(&event_json)?;
            
            return Ok(RelayMessage::Event {
                _subscription_id: subscription_id,
                event: event,
            });
        }
        
        "EOSE" => {
            // ["EOSE", subscription_id]
            let subscription_id: String;
            if parsed[1].is_string() {
                subscription_id = parsed[1].as_str().unwrap().to_string();
            } else {
                return Err(String::from("Missing subscription_id in EOSE"));
            }
            
            return Ok(RelayMessage::EndOfStoredEvents {
                _subscription_id: subscription_id,
            });
        }
        
        "NOTICE" => {
            // ["NOTICE", message]
            let notice_message: String;
            if parsed[1].is_string() {
                notice_message = parsed[1].as_str().unwrap().to_string();
            } else {
                notice_message = String::from("Unknown notice");
            }
            
            return Ok(RelayMessage::Notice {
                message: notice_message,
            });
        }
        
        "OK" => {
            // ["OK", event_id, success_bool, message]
            let event_id: String;
            if parsed[1].is_string() {
                event_id = parsed[1].as_str().unwrap().to_string();
            } else {
                event_id = String::new();
            }
            
            let success = parsed[2].as_bool().unwrap_or(false);
            
            let ok_message: String;
            if parsed[3].is_string() {
                ok_message = parsed[3].as_str().unwrap().to_string();
            } else {
                ok_message = String::new();
            }
            
            return Ok(RelayMessage::Ok {
                event_id: event_id,
                success: success,
                message: ok_message,
            });
        }
        
        _ => {
            return Ok(RelayMessage::Unknown {
                _raw: message.to_string(),
            });
        }
    }
}

/// Message sent from async relay stream to the UI forwarder.
pub enum StreamMessage {
    Event(nostr::Event),
    Eose,
    Notice(String),
}

/// Parse a single relay message using Actson (push feeder, pull events).
/// Each complete message is one WebSocket frame; we push its bytes and pull events to build RelayMessage.
pub fn parse_relay_message_actson(message: &str) -> Result<RelayMessage, String> {
    let bytes = message.as_bytes();
    let feeder = SliceJsonFeeder::new(bytes);
    let mut parser = JsonParser::new(feeder);

    let mut depth: i32 = 0;
    let mut top_level_index: i32 = -1;
    let mut msg_type: Option<String> = None;
    let mut second_str: Option<String> = None;
    // EVENT: third element is event object
    let mut sub_id: Option<String> = None;
    // OK: third is bool, fourth is string
    let mut ok_event_id: Option<String> = None;
    let mut ok_success: bool = false;
    let mut ok_message: Option<String> = None;
    // Event object state (when parsing ["EVENT", sub_id, { ... }])
    let mut current_field: Option<String> = None;
    let mut event_id: Option<String> = None;
    let mut event_pubkey: Option<String> = None;
    let mut event_created_at: u64 = 0;
    let mut event_kind: u32 = 0;
    let mut event_content: String = String::new();
    let mut event_sig: Option<String> = None;
    let mut event_tags: Vec<Vec<String>> = Vec::new();
    let mut current_tag: Vec<String> = Vec::new();
    let mut tags_depth: i32 = 0; // 0 = not in tags, 1 = in tags array, 2 = in one tag array

    loop {
        let event = match parser.next_event() {
            Ok(Some(ev)) => ev,
            Ok(None) => break,
            Err(e) => return Err(format!("Relay message parse error: {}", e)),
        };

        match event {
            JsonEvent::NeedMoreInput => {
                // SliceJsonFeeder has the full slice; we don't push more. Should not happen for a complete message.
                break;
            }
            JsonEvent::StartArray => {
                depth += 1;
                if depth == 1 {
                    top_level_index = 0;
                } else if tags_depth == 1 {
                    tags_depth = 2; // entering the tags array
                } else if tags_depth == 2 {
                    current_tag.clear(); // entering one tag array
                }
            }
            JsonEvent::EndArray => {
                if tags_depth == 2 && depth == 4 {
                    if !current_tag.is_empty() {
                        event_tags.push(current_tag.clone());
                    }
                    current_tag.clear();
                } else if tags_depth == 2 && depth == 3 {
                    tags_depth = 0; // leaving the tags array
                }
                depth -= 1;
            }
            JsonEvent::StartObject => {
                depth += 1;
                if depth == 1 {
                    top_level_index += 1;
                }
                if depth == 2 && msg_type.as_deref() == Some("EVENT") {
                    // Reset event fields for this object
                    current_field = None;
                    event_id = None;
                    event_pubkey = None;
                    event_created_at = 0;
                    event_kind = 0;
                    event_content.clear();
                    event_sig = None;
                    event_tags.clear();
                }
            }
            JsonEvent::EndObject => {
                depth -= 1;
                if depth == 1 && msg_type.as_deref() == Some("EVENT") && second_str.is_some() {
                    // Finished the event object; we have all fields (or defaults). Build RelayMessage and return.
                    let sub_id_owned = sub_id.clone().unwrap_or_default();
                    let event = nostr::Event {
                        id: event_id.unwrap_or_default(),
                        pubkey: event_pubkey.unwrap_or_default(),
                        created_at: event_created_at,
                        kind: event_kind,
                        tags: event_tags.clone(),
                        content: event_content.clone(),
                        sig: event_sig.unwrap_or_default(),
                    };
                    return Ok(RelayMessage::Event {
                        _subscription_id: sub_id_owned,
                        event,
                    });
                }
            }
            JsonEvent::FieldName => {
                if let Ok(s) = parser.current_str() {
                    current_field = Some(s.to_string());
                    if depth == 2 && s == "tags" {
                        tags_depth = 1; // next StartArray is the tags array
                    }
                }
            }
            JsonEvent::ValueString => {
                let s = parser.current_str().map(|x| x.to_string()).unwrap_or_default();
                if depth == 1 {
                    top_level_index += 1;
                    if top_level_index == 1 {
                        msg_type = Some(s);
                    } else if top_level_index == 2 {
                        second_str = Some(s.clone());
                        sub_id = Some(s.clone()); // EVENT/EOSE
                        ok_event_id = Some(s); // OK
                    } else if top_level_index == 4 && msg_type.as_deref() == Some("OK") {
                        ok_message = Some(s);
                    }
                } else if depth >= 2 && tags_depth == 0 {
                    if tags_depth == 2 {
                        current_tag.push(s);
                    } else if let Some(ref f) = current_field {
                        match f.as_str() {
                            "id" => event_id = Some(s),
                            "pubkey" => event_pubkey = Some(s),
                            "content" => event_content = s,
                            "sig" => event_sig = Some(s),
                            _ => {}
                        }
                    }
                }
            }
            JsonEvent::ValueInt => {
                if depth == 2 {
                    if let Some(ref f) = current_field {
                        if f == "created_at" {
                            if let Ok(n) = parser.current_int::<i64>() {
                                event_created_at = n.max(0) as u64;
                            }
                        } else if f == "kind" {
                            if let Ok(n) = parser.current_int::<i32>() {
                                event_kind = n.max(0) as u32;
                            }
                        }
                    }
                } else if depth == 1 && top_level_index == 2 && msg_type.as_deref() == Some("OK") {
                    // OK has no integer at index 2 (it's bool); skip
                }
            }
            JsonEvent::ValueFloat => {
                if depth == 2 {
                    if let Some(ref f) = current_field {
                        if f == "created_at" {
                            if let Ok(n) = parser.current_float() {
                                event_created_at = n.max(0.0) as u64;
                            }
                        } else if f == "kind" {
                            if let Ok(n) = parser.current_float() {
                                event_kind = n.max(0.0) as u32;
                            }
                        }
                    }
                }
            }
            JsonEvent::ValueTrue | JsonEvent::ValueFalse => {
                if depth == 1 {
                    top_level_index += 1;
                    if top_level_index == 3 && msg_type.as_deref() == Some("OK") {
                        ok_success = matches!(event, JsonEvent::ValueTrue);
                    }
                }
            }
            JsonEvent::ValueNull => {}
        }
    }

    // End of input: if we have msg_type and second_str we can build non-EVENT messages
    match msg_type.as_deref() {
        Some("EOSE") => Ok(RelayMessage::EndOfStoredEvents {
            _subscription_id: second_str.unwrap_or_default(),
        }),
        Some("NOTICE") => Ok(RelayMessage::Notice {
            message: second_str.unwrap_or_else(|| "Unknown notice".to_string()),
        }),
        Some("OK") => Ok(RelayMessage::Ok {
            event_id: ok_event_id.unwrap_or_default(),
            success: ok_success,
            message: ok_message.unwrap_or_default(),
        }),
        _ => Ok(RelayMessage::Unknown {
            _raw: message.to_string(),
        }),
    }
}

/// Run one relay's feed stream over tokio-tungstenite. Pushes each WebSocket message into
/// an Actson parser and pulls relay messages; sends events (and EOSE) to `tx`.
pub async fn run_relay_feed_stream(
    relay_url: String,
    filter: nostr::Filter,
    timeout_seconds: u32,
    tx: mpsc::UnboundedSender<StreamMessage>,
) {
    let url = match Url::parse(&relay_url) {
        Ok(u) => u,
        Err(e) => {
            let _ = tx.send(StreamMessage::Notice(format!("Invalid URL {}: {}", relay_url, e)));
            return;
        }
    };

    let (ws_stream, _) = match connect_async(&url).await {
        Ok(t) => t,
        Err(e) => {
            println!("Failed to connect to {}: {}", relay_url, e);
            return;
        }
    };

    println!("Connected to {}", relay_url);

    let (mut write, mut read) = ws_stream.split();
    let subscription_id = format!(
        "plume_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );

    let filter_json = nostr::filter_to_json(&filter);
    let req_message = format!("[\"REQ\",\"{}\",{}]", subscription_id, filter_json);

    if write.send(WsMessage::Text(req_message)).await.is_err() {
        return;
    }

    let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(timeout_seconds as u64);

    loop {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        let timeout = tokio::time::timeout(
            tokio::time::Duration::from_secs(1),
            read.next(),
        );
        match timeout.await {
            Ok(Some(Ok(WsMessage::Text(text)))) => {
                match parse_relay_message_actson(&text) {
                    Ok(RelayMessage::Event { event, .. }) => {
                        if tx.send(StreamMessage::Event(event)).is_err() {
                            break;
                        }
                    }
                    Ok(RelayMessage::EndOfStoredEvents { .. }) => {
                        break;
                    }
                    Ok(RelayMessage::Notice { message }) => {
                        println!("Notice from {}: {}", relay_url, message);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        println!("Parse error from {}: {}", relay_url, e);
                    }
                }
            }
            Ok(Some(Ok(WsMessage::Close(_)))) | Ok(Some(Err(_))) => break,
            Ok(Some(Ok(_))) => {} // Ping/Pong/Binary, ignore
            Ok(None) => break,
            Err(_) => {} // timeout, loop again
        }
    }

    let _ = tx.send(StreamMessage::Eose);
}

/// Run a long-lived DM subscription (kind 4) with two filters (received + sent). Does not exit on EOSE.
pub async fn run_relay_dm_stream(
    relay_url: String,
    filter_received: nostr::Filter,
    filter_sent: nostr::Filter,
    tx: mpsc::UnboundedSender<StreamMessage>,
) {
    let url = match Url::parse(&relay_url) {
        Ok(u) => u,
        Err(e) => {
            let _ = tx.send(StreamMessage::Notice(format!("Invalid URL {}: {}", relay_url, e)));
            return;
        }
    };

    let (ws_stream, _) = match connect_async(&url).await {
        Ok(t) => t,
        Err(e) => {
            println!("DM stream: failed to connect to {}: {}", relay_url, e);
            return;
        }
    };

    let (mut write, mut read) = ws_stream.split();
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

    if write.send(WsMessage::Text(req_message)).await.is_err() {
        return;
    }

    loop {
        let timeout = tokio::time::timeout(
            tokio::time::Duration::from_secs(60),
            read.next(),
        );
        match timeout.await {
            Ok(Some(Ok(WsMessage::Text(text)))) => {
                match parse_relay_message_actson(&text) {
                    Ok(RelayMessage::Event { event, .. }) => {
                        if event.kind == nostr::KIND_DM {
                            if tx.send(StreamMessage::Event(event)).is_err() {
                                break;
                            }
                        }
                    }
                    Ok(RelayMessage::EndOfStoredEvents { .. }) => {
                        // Keep connection open for new DMs
                    }
                    Ok(RelayMessage::Notice { message }) => {
                        println!("DM stream {} notice: {}", relay_url, message);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        println!("DM stream {} parse error: {}", relay_url, e);
                    }
                }
            }
            Ok(Some(Ok(WsMessage::Close(_)))) | Ok(Some(Err(_))) => break,
            Ok(Some(Ok(_))) => {}
            Ok(None) => break,
            Err(_) => {} // timeout, continue
        }
    }
}

// Fetch notes from a relay (simple blocking function)
// Returns a vector of events
pub fn fetch_notes_from_relay(
    relay_url: &str,
    filter: &nostr::Filter,
    timeout_seconds: u32,
) -> Result<Vec<nostr::Event>, String> {
    // Create and connect to the relay
    let mut relay = RelayConnection::new(relay_url);
    relay.connect()?;
    
    // Generate a unique subscription ID
    let subscription_id = format!("plume_{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis());
    
    // Subscribe with the filter
    relay.subscribe(&subscription_id, filter)?;
    
    // Collect events until EOSE or timeout
    let mut events: Vec<nostr::Event> = Vec::new();
    let start_time = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(timeout_seconds as u64);
    
    loop {
        // Check for timeout
        if start_time.elapsed() > timeout {
            println!("Timeout reached, closing subscription");
            break;
        }
        
        // Try to receive a message
        match relay.receive() {
            Ok(message) => {
                match parse_relay_message(&message) {
                    Ok(RelayMessage::Event { _subscription_id: _, event }) => {
                        events.push(event);
                    }
                    Ok(RelayMessage::EndOfStoredEvents { _subscription_id: _ }) => {
                        println!("Received EOSE, done fetching stored events");
                        break;
                    }
                    Ok(RelayMessage::Notice { message }) => {
                        println!("Notice from relay: {}", message);
                    }
                    Ok(_) => {
                        // Ignore other message types
                    }
                    Err(e) => {
                        println!("Error parsing relay message: {}", e);
                    }
                }
            }
            Err(e) => {
                // Check if it's just a ping/pong
                if e.contains("ping") || e.contains("pong") {
                    continue;
                }
                println!("Error receiving from relay: {}", e);
                break;
            }
        }
    }
    
    // Clean up
    let _ = relay.close_subscription(&subscription_id);
    relay.disconnect();
    
    println!("Fetched {} events from {}", events.len(), relay_url);
    return Ok(events);
}

// Fetch profile metadata for a public key from a relay
// Returns the profile if found, or None if not found
pub fn fetch_profile_from_relay(
    relay_url: &str,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Option<nostr::ProfileMetadata>, String> {
    // Create filter for kind 0 (metadata) from this author
    let filter = nostr::filter_profile_by_author(pubkey);
    
    // Fetch events (should be at most 1)
    let events = fetch_notes_from_relay(relay_url, &filter, timeout_seconds)?;
    
    // Find the most recent kind 0 event
    let mut best_event: Option<&nostr::Event> = None;
    
    for event in &events {
        if event.kind == nostr::KIND_METADATA {
            match &best_event {
                None => {
                    best_event = Some(event);
                }
                Some(current) => {
                    // Keep the more recent one
                    if event.created_at > current.created_at {
                        best_event = Some(event);
                    }
                }
            }
        }
    }
    
    // Parse the profile from the event content
    match best_event {
        Some(event) => {
            match nostr::parse_profile(&event.content) {
                Ok(mut profile) => {
                    profile.created_at = Some(event.created_at);
                    println!("Found profile for {}", pubkey);
                    return Ok(Some(profile));
                }
                Err(e) => {
                    println!("Failed to parse profile: {}", e);
                    return Err(e);
                }
            }
        }
        None => {
            println!("No profile found for {}", pubkey);
            return Ok(None);
        }
    }
}

// Fetch profile from multiple relays (tries each until one succeeds)
pub fn fetch_profile_from_relays(
    relay_urls: &Vec<String>,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Option<nostr::ProfileMetadata>, String> {
    // Try each relay until we find a profile
    for relay_url in relay_urls {
        match fetch_profile_from_relay(relay_url, pubkey, timeout_seconds) {
            Ok(Some(profile)) => {
                return Ok(Some(profile));
            }
            Ok(None) => {
                // No profile on this relay, try next
                continue;
            }
            Err(e) => {
                println!("Error fetching profile from {}: {}", relay_url, e);
                // Try next relay
                continue;
            }
        }
    }
    
    // No profile found on any relay
    return Ok(None);
}

// ============================================================
// Following / Followers
// ============================================================

// Fetch a user's contact list (who they follow) from a relay
pub fn fetch_following_from_relay(
    relay_url: &str,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Option<nostr::ContactList>, String> {
    // Create filter for kind 3 (contact list) from this author
    let filter = nostr::filter_contact_list_by_author(pubkey);
    
    // Fetch events
    let events = fetch_notes_from_relay(relay_url, &filter, timeout_seconds)?;
    
    // Find the most recent kind 3 event
    let mut best_event: Option<&nostr::Event> = None;
    
    for event in &events {
        if event.kind == nostr::KIND_CONTACTS {
            match &best_event {
                None => {
                    best_event = Some(event);
                }
                Some(current) => {
                    if event.created_at > current.created_at {
                        best_event = Some(event);
                    }
                }
            }
        }
    }
    
    // Parse the contact list from the event
    match best_event {
        Some(event) => {
            match nostr::parse_contact_list(event) {
                Ok(contact_list) => {
                    println!("Found contact list for {} with {} contacts", 
                             pubkey, contact_list.contacts.len());
                    return Ok(Some(contact_list));
                }
                Err(e) => {
                    println!("Failed to parse contact list: {}", e);
                    return Err(e);
                }
            }
        }
        None => {
            println!("No contact list found for {}", pubkey);
            return Ok(None);
        }
    }
}

// Fetch following from multiple relays
pub fn fetch_following_from_relays(
    relay_urls: &Vec<String>,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Option<nostr::ContactList>, String> {
    // Try each relay until we find a contact list
    for relay_url in relay_urls {
        match fetch_following_from_relay(relay_url, pubkey, timeout_seconds) {
            Ok(Some(contact_list)) => {
                return Ok(Some(contact_list));
            }
            Ok(None) => {
                continue;
            }
            Err(e) => {
                println!("Error fetching following from {}: {}", relay_url, e);
                continue;
            }
        }
    }
    
    return Ok(None);
}

// Fetch followers (who follows a user) from a relay
// This searches for kind 3 events that have a "p" tag for the target pubkey
pub fn fetch_followers_from_relay(
    relay_url: &str,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Vec<nostr::FollowerInfo>, String> {
    // Create filter for kind 3 events that tag this pubkey
    let filter = nostr::filter_followers_by_pubkey(pubkey);
    
    // Fetch events
    let events = fetch_notes_from_relay(relay_url, &filter, timeout_seconds)?;
    
    // Extract unique follower pubkeys
    // We need to dedupe because someone might have multiple contact list versions
    let mut seen_pubkeys: Vec<String> = Vec::new();
    let mut followers: Vec<nostr::FollowerInfo> = Vec::new();
    
    for event in &events {
        // The author of a kind 3 event that tags our pubkey is a follower
        if !seen_pubkeys.contains(&event.pubkey) {
            seen_pubkeys.push(event.pubkey.clone());
            followers.push(nostr::FollowerInfo {
                pubkey: event.pubkey.clone(),
            });
        }
    }
    
    println!("Found {} followers for {} on {}", followers.len(), pubkey, relay_url);
    return Ok(followers);
}

// Fetch followers from multiple relays and combine results
pub fn fetch_followers_from_relays(
    relay_urls: &Vec<String>,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Vec<nostr::FollowerInfo>, String> {
    let mut all_followers: Vec<nostr::FollowerInfo> = Vec::new();
    let mut seen_pubkeys: Vec<String> = Vec::new();
    
    // Fetch from each relay and combine
    for relay_url in relay_urls {
        match fetch_followers_from_relay(relay_url, pubkey, timeout_seconds) {
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
    
    println!("Total {} unique followers found", all_followers.len());
    return Ok(all_followers);
}

// ============================================================
// Relay list (NIP-65 kind 10002)
// ============================================================

/// Fetch a user's relay list (kind 10002) from a single relay.
pub fn fetch_relay_list_from_relay(
    relay_url: &str,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Option<Vec<String>>, String> {
    let filter = nostr::filter_relay_list_by_author(pubkey);
    let events = fetch_notes_from_relay(relay_url, &filter, timeout_seconds)?;
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

/// Fetch a user's relay list from multiple relays (returns first non-empty list found).
pub fn fetch_relay_list_from_relays(
    relay_urls: &Vec<String>,
    pubkey: &str,
    timeout_seconds: u32,
) -> Result<Vec<String>, String> {
    for relay_url in relay_urls {
        match fetch_relay_list_from_relay(relay_url, pubkey, timeout_seconds) {
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
// Event Publishing
// ============================================================

// Result of publishing an event to a relay
pub struct PublishResult {
    pub relay_url: String,
    pub success: bool,
    pub message: String,
}

// Publish an event to a single relay and wait for OK response
pub fn publish_event_to_relay(
    relay_url: &str,
    event: &nostr::Event,
    timeout_seconds: u32,
) -> Result<PublishResult, String> {
    // Connect to relay
    let mut relay = RelayConnection::new(relay_url);
    relay.connect()?;
    
    // Publish the event
    relay.publish_event(event)?;
    
    // Wait for OK response
    let start_time = std::time::Instant::now();
    let timeout = std::time::Duration::from_secs(timeout_seconds as u64);
    
    loop {
        // Check for timeout
        if start_time.elapsed() > timeout {
            relay.disconnect();
            return Ok(PublishResult {
                relay_url: relay_url.to_string(),
                success: false,
                message: String::from("Timeout waiting for response"),
            });
        }
        
        // Try to receive a message
        match relay.receive() {
            Ok(message) => {
                match parse_relay_message(&message) {
                    Ok(RelayMessage::Ok { event_id, success, message }) => {
                        // Check if this OK is for our event
                        if event_id == event.id {
                            relay.disconnect();
                            return Ok(PublishResult {
                                relay_url: relay_url.to_string(),
                                success: success,
                                message: message,
                            });
                        }
                    }
                    Ok(RelayMessage::Notice { message }) => {
                        println!("Notice from {}: {}", relay_url, message);
                        // Check if it's an error notice about our event
                        if message.contains(&event.id) || message.to_lowercase().contains("error") {
                            relay.disconnect();
                            return Ok(PublishResult {
                                relay_url: relay_url.to_string(),
                                success: false,
                                message: message,
                            });
                        }
                    }
                    Ok(_) => {
                        // Ignore other messages
                    }
                    Err(e) => {
                        println!("Error parsing message: {}", e);
                    }
                }
            }
            Err(e) => {
                if e.contains("ping") || e.contains("pong") {
                    continue;
                }
                relay.disconnect();
                return Ok(PublishResult {
                    relay_url: relay_url.to_string(),
                    success: false,
                    message: e,
                });
            }
        }
    }
}

// Publish an event to multiple relays
pub fn publish_event_to_relays(
    relay_urls: &Vec<String>,
    event: &nostr::Event,
    timeout_seconds: u32,
) -> Vec<PublishResult> {
    let mut results: Vec<PublishResult> = Vec::new();
    
    for relay_url in relay_urls {
        match publish_event_to_relay(relay_url, event, timeout_seconds) {
            Ok(result) => {
                println!("Publish to {}: success={}, message={}", 
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

// Convert publish results to JSON
pub fn publish_results_to_json(results: &Vec<PublishResult>) -> String {
    let mut json = String::new();
    json.push_str("{");
    
    // Count successes
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

// Escape JSON string
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

