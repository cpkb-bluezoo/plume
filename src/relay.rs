// Plume - Relay Connection Management
// Handles WebSocket connections to Nostr relays

use std::net::TcpStream;
use tungstenite::{connect, stream::MaybeTlsStream, Message, WebSocket};
use url::Url;

use crate::nostr;

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
    Event { subscription_id: String, event: nostr::Event },
    EndOfStoredEvents { subscription_id: String },
    Notice { message: String },
    Ok { event_id: String, success: bool, message: String },
    Unknown { raw: String },
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
                subscription_id: subscription_id,
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
                subscription_id: subscription_id,
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
                raw: message.to_string(),
            });
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
                    Ok(RelayMessage::Event { subscription_id: _, event }) => {
                        events.push(event);
                    }
                    Ok(RelayMessage::EndOfStoredEvents { subscription_id: _ }) => {
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
                Ok(profile) => {
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

