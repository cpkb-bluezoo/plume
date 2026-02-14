/*
 * config.rs
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

use std::fs;
use std::io;
use std::path::Path;

use bytes::BytesMut;
use crate::json::{JsonContentHandler, JsonNumber, JsonParser};

// The main configuration structure
pub struct Config {
    pub public_key: String,
    pub private_key: Option<String>,
    pub relays: Vec<String>,
    pub display_name: String,
    pub profile_picture: Option<String>,
    pub profile_metadata: Option<String>,
    pub home_feed_mode: String,
    pub media_server_url: String,
    pub muted_users: Vec<String>,
    pub muted_words: Vec<String>,
    pub muted_hashtags: Vec<String>,
    pub bookmarks: Vec<String>,
    pub default_zap_amount: u32,
}

impl Config {
    pub fn new() -> Config {
        Config {
            public_key: String::new(),
            private_key: None,
            relays: vec![
                String::from("wss://relay.damus.io"),
                String::from("wss://nos.lol"),
                String::from("wss://relay.nostr.band"),
            ],
            display_name: String::from("Anonymous"),
            profile_picture: None,
            profile_metadata: None,
            home_feed_mode: String::from("firehose"),
            media_server_url: String::from("https://blossom.primal.net"),
            muted_users: Vec::new(),
            muted_words: Vec::new(),
            muted_hashtags: Vec::new(),
            bookmarks: Vec::new(),
            default_zap_amount: 42,
        }
    }
}

// ============================================================
// Push-parser handler for Config
// ============================================================

/// Which string array field we're currently inside (depth 2).
#[derive(Clone, Copy, PartialEq)]
enum ConfigArrayField {
    None,
    Relays,
    MutedUsers,
    MutedWords,
    MutedHashtags,
    Bookmarks,
}

struct ConfigHandler {
    depth: i32,
    current_field: Option<String>,
    array_field: ConfigArrayField,
    // Scalar fields
    public_key: String,
    private_key: Option<String>,
    display_name: String,
    profile_picture: Option<String>,
    profile_metadata: Option<String>,
    home_feed_mode: String,
    media_server_url: String,
    default_zap_amount: u32,
    // Array fields
    relays: Vec<String>,
    muted_users: Vec<String>,
    muted_words: Vec<String>,
    muted_hashtags: Vec<String>,
    bookmarks: Vec<String>,
}

impl ConfigHandler {
    fn new() -> Self {
        Self {
            depth: 0,
            current_field: None,
            array_field: ConfigArrayField::None,
            public_key: String::new(),
            private_key: None,
            display_name: String::from("Anonymous"),
            profile_picture: None,
            profile_metadata: None,
            home_feed_mode: String::from("firehose"),
            media_server_url: String::from("https://blossom.primal.net"),
            default_zap_amount: 42,
            relays: Vec::new(),
            muted_users: Vec::new(),
            muted_words: Vec::new(),
            muted_hashtags: Vec::new(),
            bookmarks: Vec::new(),
        }
    }

    fn take_config(self) -> Config {
        let mut relays = self.relays;
        if relays.is_empty() {
            relays.push(String::from("wss://relay.damus.io"));
            relays.push(String::from("wss://nos.lol"));
            relays.push(String::from("wss://relay.nostr.band"));
        }
        let home_feed_mode = if self.home_feed_mode == "follows" {
            String::from("follows")
        } else {
            String::from("firehose")
        };
        Config {
            public_key: self.public_key,
            private_key: self.private_key,
            relays,
            display_name: self.display_name,
            profile_picture: self.profile_picture,
            profile_metadata: self.profile_metadata,
            home_feed_mode,
            media_server_url: self.media_server_url,
            default_zap_amount: self.default_zap_amount,
            muted_users: self.muted_users,
            muted_words: self.muted_words,
            muted_hashtags: self.muted_hashtags,
            bookmarks: self.bookmarks,
        }
    }
}

impl JsonContentHandler for ConfigHandler {
    fn start_object(&mut self) {
        self.depth += 1;
    }
    fn end_object(&mut self) {
        self.depth -= 1;
    }
    fn start_array(&mut self) {
        self.depth += 1;
        if self.depth == 2 {
            if let Some(ref f) = self.current_field {
                self.array_field = match f.as_str() {
                    "relays" => ConfigArrayField::Relays,
                    "muted_users" => ConfigArrayField::MutedUsers,
                    "muted_words" => ConfigArrayField::MutedWords,
                    "muted_hashtags" => ConfigArrayField::MutedHashtags,
                    "bookmarks" => ConfigArrayField::Bookmarks,
                    _ => ConfigArrayField::None,
                };
            }
        }
    }
    fn end_array(&mut self) {
        if self.depth == 2 {
            self.array_field = ConfigArrayField::None;
        }
        self.depth -= 1;
    }

    fn key(&mut self, key: &str) {
        self.current_field = Some(key.to_string());
    }

    fn string_value(&mut self, value: &str) {
        // Inside an array at depth 2
        if self.depth == 2 && self.array_field != ConfigArrayField::None {
            let vec = match self.array_field {
                ConfigArrayField::Relays => &mut self.relays,
                ConfigArrayField::MutedUsers => &mut self.muted_users,
                ConfigArrayField::MutedWords => &mut self.muted_words,
                ConfigArrayField::MutedHashtags => &mut self.muted_hashtags,
                ConfigArrayField::Bookmarks => &mut self.bookmarks,
                ConfigArrayField::None => return,
            };
            vec.push(value.to_string());
            return;
        }
        // Top-level scalar
        if self.depth == 1 {
            if let Some(ref f) = self.current_field {
                match f.as_str() {
                    "public_key" => self.public_key = value.to_string(),
                    "private_key" => self.private_key = Some(value.to_string()),
                    "display_name" => self.display_name = value.to_string(),
                    "profile_picture" => self.profile_picture = Some(value.to_string()),
                    "profile_metadata" => self.profile_metadata = Some(value.to_string()),
                    "home_feed_mode" => self.home_feed_mode = value.to_string(),
                    "media_server_url" => self.media_server_url = value.to_string(),
                    _ => {}
                }
            }
        }
    }

    fn number_value(&mut self, number: JsonNumber) {
        if self.depth == 1 {
            if let Some(ref f) = self.current_field {
                if f == "default_zap_amount" {
                    let n = number.as_f64() as u32;
                    if n >= 1 && n <= 1_000_000 {
                        self.default_zap_amount = n;
                    }
                }
            }
        }
    }

    fn boolean_value(&mut self, _value: bool) {}
    fn null_value(&mut self) {}
}

// ============================================================
// Serialization / Deserialization
// ============================================================

pub fn config_to_json(config: &Config) -> String {
    let mut json = String::new();
    json.push_str("{\n");
    
    json.push_str("  \"public_key\": \"");
    json.push_str(&escape_json_string(&config.public_key));
    json.push_str("\",\n");
    
    json.push_str("  \"private_key\": ");
    match &config.private_key {
        Some(key) => {
            json.push_str("\"");
            json.push_str(&escape_json_string(key));
            json.push_str("\"");
        }
        None => json.push_str("null"),
    }
    json.push_str(",\n");
    
    json.push_str("  \"relays\": [\n");
    for (index, relay) in config.relays.iter().enumerate() {
        json.push_str("    \"");
        json.push_str(&escape_json_string(relay));
        json.push_str("\"");
        if index < config.relays.len() - 1 { json.push_str(","); }
        json.push_str("\n");
    }
    json.push_str("  ],\n");

    json.push_str("  \"display_name\": \"");
    json.push_str(&escape_json_string(&config.display_name));
    json.push_str("\",\n");

    json.push_str("  \"profile_picture\": ");
    match &config.profile_picture {
        Some(url) => { json.push_str("\""); json.push_str(&escape_json_string(url)); json.push_str("\""); }
        None => json.push_str("null"),
    }
    json.push_str(",\n");

    json.push_str("  \"profile_metadata\": ");
    match &config.profile_metadata {
        Some(s) => { json.push_str("\""); json.push_str(&escape_json_string(s)); json.push_str("\""); }
        None => json.push_str("null"),
    }
    json.push_str(",\n");

    json.push_str("  \"home_feed_mode\": \"");
    json.push_str(&escape_json_string(&config.home_feed_mode));
    json.push_str("\",\n");

    json.push_str("  \"media_server_url\": \"");
    json.push_str(&escape_json_string(&config.media_server_url));
    json.push_str("\",\n");

    write_string_array(&mut json, "muted_users", &config.muted_users);
    json.push_str(",\n");
    write_string_array(&mut json, "muted_words", &config.muted_words);
    json.push_str(",\n");
    write_string_array(&mut json, "muted_hashtags", &config.muted_hashtags);
    json.push_str(",\n");
    write_string_array(&mut json, "bookmarks", &config.bookmarks);
    json.push_str(",\n");

    json.push_str("  \"default_zap_amount\": ");
    json.push_str(&config.default_zap_amount.to_string());
    json.push_str("\n");

    json.push_str("}");
    return json;
}

fn write_string_array(json: &mut String, name: &str, items: &[String]) {
    json.push_str("  \"");
    json.push_str(name);
    json.push_str("\": [\n");
    for (i, s) in items.iter().enumerate() {
        json.push_str("    \"");
        json.push_str(&escape_json_string(s));
        json.push_str("\"");
        if i < items.len() - 1 { json.push_str(","); }
        json.push_str("\n");
    }
    json.push_str("  ]");
}

pub fn json_to_config(json_str: &str) -> Result<Config, String> {
    let mut handler = ConfigHandler::new();
    let mut parser = JsonParser::new();
    let mut buf = BytesMut::from(json_str.as_bytes());
    parser.receive(&mut buf, &mut handler).map_err(|e| format!("Invalid JSON: {}", e))?;
    parser.close(&mut handler).map_err(|e| format!("Invalid JSON: {}", e))?;
    Ok(handler.take_config())
}

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
// File System
// ============================================================

pub fn get_config_dir() -> Option<String> {
    match dirs::home_dir() {
        Some(home) => {
            let config_path = home.join(".plume");
            config_path.to_str().map(String::from)
        }
        None => None,
    }
}

pub fn ensure_config_dir(config_dir: &str) -> Result<(), io::Error> {
    let path = Path::new(config_dir);
    if path.exists() {
        return Ok(());
    }
    fs::create_dir_all(path)?;
    println!("Created config directory: {}", config_dir);
    return Ok(());
}

fn get_config_file_path(config_dir: &str) -> String {
    Path::new(config_dir).join("config.json").to_string_lossy().to_string()
}

pub fn load_config(config_dir: &str) -> Result<Config, String> {
    let config_file = get_config_file_path(config_dir);
    let path = Path::new(&config_file);
    if !path.exists() {
        println!("No config file found, using defaults");
        return Ok(Config::new());
    }
    let contents = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => return Err(format!("Could not read config file: {}", e)),
    };
    return json_to_config(&contents);
}

pub fn save_config(config_dir: &str, config: &Config) -> Result<(), String> {
    let config_file = get_config_file_path(config_dir);
    let json = config_to_json(config);
    match fs::write(&config_file, json) {
        Ok(()) => {
            println!("Saved config to: {}", config_file);
            return Ok(());
        }
        Err(e) => {
            return Err(format!("Could not write config file: {}", e));
        }
    }
}
