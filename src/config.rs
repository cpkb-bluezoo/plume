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

use crate::debug_log;

use bytes::BytesMut;
use crate::json::{JsonContentHandler, JsonNumber, JsonParser};
use crate::nostr;

// The main configuration structure.
// Profile fields (name, about, picture, nip05, banner, website, lud16) are stored
// directly rather than embedded as a JSON string, matching the Nostr kind 0 field names.
pub struct Config {
    pub public_key: String,
    pub private_key: Option<String>,
    pub relays: Vec<String>,
    // Profile fields (Nostr kind 0)
    pub name: String,
    pub about: Option<String>,
    pub picture: Option<String>,
    pub nip05: Option<String>,
    pub banner: Option<String>,
    pub website: Option<String>,
    pub lud16: Option<String>,
    // App settings
    pub home_feed_mode: String,
    pub media_server_url: String,
    pub following: Vec<String>,
    pub muted_users: Vec<String>,
    pub muted_words: Vec<String>,
    pub muted_hashtags: Vec<String>,
    pub bookmarks: Vec<String>,
    pub default_zap_amount: u32,
    pub hide_encrypted_notes: bool,
    /// Unix timestamp of the last time the user read their DMs.
    /// Messages with created_at > this value are considered unread.
    pub dm_last_read_at: u64,
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
            name: String::from("Anonymous"),
            about: None,
            picture: None,
            nip05: None,
            banner: None,
            website: None,
            lud16: None,
            home_feed_mode: String::from("firehose"),
            media_server_url: String::from("https://blossom.primal.net"),
            following: Vec::new(),
            muted_users: Vec::new(),
            muted_words: Vec::new(),
            muted_hashtags: Vec::new(),
            bookmarks: Vec::new(),
            default_zap_amount: 42,
            hide_encrypted_notes: true,
            dm_last_read_at: 0,
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
    Following,
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
    name: String,
    about: Option<String>,
    picture: Option<String>,
    nip05: Option<String>,
    banner: Option<String>,
    website: Option<String>,
    lud16: Option<String>,
    home_feed_mode: String,
    media_server_url: String,
    default_zap_amount: u32,
    hide_encrypted_notes: bool,
    dm_last_read_at: u64,
    // Array fields
    relays: Vec<String>,
    following: Vec<String>,
    muted_users: Vec<String>,
    muted_words: Vec<String>,
    muted_hashtags: Vec<String>,
    bookmarks: Vec<String>,
    // Legacy field for backward compatibility (old configs stored profile as embedded JSON string)
    profile_metadata_raw: Option<String>,
}

impl ConfigHandler {
    fn new() -> Self {
        Self {
            depth: 0,
            current_field: None,
            array_field: ConfigArrayField::None,
            public_key: String::new(),
            private_key: None,
            name: String::from("Anonymous"),
            about: None,
            picture: None,
            nip05: None,
            banner: None,
            website: None,
            lud16: None,
            home_feed_mode: String::from("firehose"),
            media_server_url: String::from("https://blossom.primal.net"),
            default_zap_amount: 42,
            hide_encrypted_notes: true,
            dm_last_read_at: 0,
            relays: Vec::new(),
            following: Vec::new(),
            muted_users: Vec::new(),
            muted_words: Vec::new(),
            muted_hashtags: Vec::new(),
            bookmarks: Vec::new(),
            profile_metadata_raw: None,
        }
    }

    fn take_config(mut self) -> Config {
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

        // Backward compatibility: if an old config had profile_metadata (embedded JSON string),
        // parse it and fill in any profile fields that are still at defaults.
        if let Some(ref raw) = self.profile_metadata_raw {
            if let Ok(profile) = nostr::parse_profile(raw) {
                if self.name == "Anonymous" {
                    if let Some(ref n) = profile.name {
                        self.name = n.clone();
                    }
                }
                if self.about.is_none() {
                    self.about = profile.about.clone();
                }
                if self.picture.is_none() {
                    self.picture = profile.picture.clone();
                }
                if self.nip05.is_none() {
                    self.nip05 = profile.nip05.clone();
                }
                if self.banner.is_none() {
                    self.banner = profile.banner.clone();
                }
                if self.website.is_none() {
                    self.website = profile.website.clone();
                }
                if self.lud16.is_none() {
                    self.lud16 = profile.lud16.clone();
                }
            }
        }

        Config {
            public_key: self.public_key,
            private_key: self.private_key,
            relays,
            name: self.name,
            about: self.about,
            picture: self.picture,
            nip05: self.nip05,
            banner: self.banner,
            website: self.website,
            lud16: self.lud16,
            home_feed_mode,
            media_server_url: self.media_server_url,
            default_zap_amount: self.default_zap_amount,
            following: self.following,
            muted_users: self.muted_users,
            muted_words: self.muted_words,
            muted_hashtags: self.muted_hashtags,
            bookmarks: self.bookmarks,
            hide_encrypted_notes: self.hide_encrypted_notes,
            dm_last_read_at: self.dm_last_read_at,
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
                    "following" => ConfigArrayField::Following,
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
                ConfigArrayField::Following => &mut self.following,
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
                    // New field names (Nostr kind 0)
                    "name" => self.name = value.to_string(),
                    "about" => self.about = Some(value.to_string()),
                    "picture" => self.picture = Some(value.to_string()),
                    "nip05" => self.nip05 = Some(value.to_string()),
                    "banner" => self.banner = Some(value.to_string()),
                    "website" => self.website = Some(value.to_string()),
                    "lud16" => self.lud16 = Some(value.to_string()),
                    // Legacy field names (old configs): map to new names
                    "display_name" => {
                        if self.name == "Anonymous" {
                            self.name = value.to_string();
                        }
                    }
                    "profile_picture" => {
                        if self.picture.is_none() {
                            self.picture = Some(value.to_string());
                        }
                    }
                    // Legacy embedded JSON string: store raw for parsing in take_config()
                    "profile_metadata" => {
                        self.profile_metadata_raw = Some(value.to_string());
                    }
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
                } else if f == "dm_last_read_at" {
                    self.dm_last_read_at = number.as_f64().max(0.0) as u64;
                }
            }
        }
    }

    fn boolean_value(&mut self, value: bool) {
        if self.depth == 1 {
            if let Some(ref f) = self.current_field {
                if f == "hide_encrypted_notes" {
                    self.hide_encrypted_notes = value;
                }
            }
        }
    }
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
        if index < config.relays.len() - 1 {
            json.push_str(",");
        }
        json.push_str("\n");
    }
    json.push_str("  ],\n");

    // Profile fields (Nostr kind 0 names)
    json.push_str("  \"name\": \"");
    json.push_str(&escape_json_string(&config.name));
    json.push_str("\",\n");

    write_optional_string(&mut json, "about", &config.about);
    json.push_str(",\n");

    write_optional_string(&mut json, "picture", &config.picture);
    json.push_str(",\n");

    write_optional_string(&mut json, "nip05", &config.nip05);
    json.push_str(",\n");

    write_optional_string(&mut json, "banner", &config.banner);
    json.push_str(",\n");

    write_optional_string(&mut json, "website", &config.website);
    json.push_str(",\n");

    write_optional_string(&mut json, "lud16", &config.lud16);
    json.push_str(",\n");

    // App settings
    json.push_str("  \"home_feed_mode\": \"");
    json.push_str(&escape_json_string(&config.home_feed_mode));
    json.push_str("\",\n");

    json.push_str("  \"media_server_url\": \"");
    json.push_str(&escape_json_string(&config.media_server_url));
    json.push_str("\",\n");

    write_string_array(&mut json, "following", &config.following);
    json.push_str(",\n");
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
    json.push_str(",\n");

    json.push_str("  \"hide_encrypted_notes\": ");
    json.push_str(if config.hide_encrypted_notes { "true" } else { "false" });
    json.push_str(",\n");

    json.push_str("  \"dm_last_read_at\": ");
    json.push_str(&config.dm_last_read_at.to_string());
    json.push_str("\n");

    json.push_str("}");
    return json;
}

fn write_optional_string(json: &mut String, name: &str, value: &Option<String>) {
    json.push_str("  \"");
    json.push_str(name);
    json.push_str("\": ");
    match value {
        Some(s) => {
            json.push_str("\"");
            json.push_str(&escape_json_string(s));
            json.push_str("\"");
        }
        None => json.push_str("null"),
    }
}

fn write_string_array(json: &mut String, name: &str, items: &[String]) {
    json.push_str("  \"");
    json.push_str(name);
    json.push_str("\": [\n");
    for (i, s) in items.iter().enumerate() {
        json.push_str("    \"");
        json.push_str(&escape_json_string(s));
        json.push_str("\"");
        if i < items.len() - 1 {
            json.push_str(",");
        }
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

pub fn escape_json_string(input: &str) -> String {
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
    debug_log!("Created config directory: {}", config_dir);
    return Ok(());
}

fn get_config_file_path(config_dir: &str) -> String {
    Path::new(config_dir).join("config.json").to_string_lossy().to_string()
}

pub fn load_config(config_dir: &str) -> Result<Config, String> {
    let config_file = get_config_file_path(config_dir);
    let path = Path::new(&config_file);
    if !path.exists() {
        debug_log!("No config file found, using defaults");
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
            debug_log!("Saved config to: {}", config_file);
            return Ok(());
        }
        Err(e) => {
            return Err(format!("Could not write config file: {}", e));
        }
    }
}

// ============================================================
// App-level configuration (plume.json) - multi-profile support
// ============================================================

pub struct AppConfig {
    pub active_profile: Option<String>,
    pub known_profiles: Vec<String>,  // list of npub strings
}

impl AppConfig {
    pub fn new() -> Self {
        Self {
            active_profile: None,
            known_profiles: Vec::new(),
        }
    }
}

struct AppConfigHandler {
    depth: i32,
    current_field: Option<String>,
    active_profile: Option<String>,
    known_profiles: Vec<String>,
    in_profiles_array: bool,
}

impl AppConfigHandler {
    fn new() -> Self {
        Self {
            depth: 0,
            current_field: None,
            active_profile: None,
            known_profiles: Vec::new(),
            in_profiles_array: false,
        }
    }

    fn take(self) -> AppConfig {
        AppConfig {
            active_profile: self.active_profile,
            known_profiles: self.known_profiles,
        }
    }
}

impl JsonContentHandler for AppConfigHandler {
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
                if f == "known_profiles" {
                    self.in_profiles_array = true;
                }
            }
        }
    }
    fn end_array(&mut self) {
        if self.depth == 2 {
            self.in_profiles_array = false;
        }
        self.depth -= 1;
    }
    fn key(&mut self, key: &str) {
        self.current_field = Some(key.to_string());
    }
    fn string_value(&mut self, value: &str) {
        // known_profiles is now a simple array of npub strings
        if self.in_profiles_array && self.depth == 2 {
            self.known_profiles.push(value.to_string());
            return;
        }
        if self.depth == 1 {
            if let Some(ref f) = self.current_field {
                if f == "active_profile" {
                    self.active_profile = Some(value.to_string());
                }
            }
        }
    }
    fn number_value(&mut self, _number: JsonNumber) {}
    fn boolean_value(&mut self, _value: bool) {}
    fn null_value(&mut self) {}
}

pub fn app_config_to_json(config: &AppConfig) -> String {
    let mut json = String::new();
    json.push_str("{\n");
    json.push_str("  \"active_profile\": ");
    match &config.active_profile {
        Some(npub) => {
            json.push_str("\"");
            json.push_str(&escape_json_string(npub));
            json.push_str("\"");
        }
        None => json.push_str("null"),
    }
    json.push_str(",\n");
    json.push_str("  \"known_profiles\": [\n");
    for (i, npub) in config.known_profiles.iter().enumerate() {
        json.push_str("    \"");
        json.push_str(&escape_json_string(npub));
        json.push_str("\"");
        if i < config.known_profiles.len() - 1 {
            json.push_str(",");
        }
        json.push_str("\n");
    }
    json.push_str("  ]\n");
    json.push_str("}");
    json
}

fn json_to_app_config(json_str: &str) -> Result<AppConfig, String> {
    let mut handler = AppConfigHandler::new();
    let mut parser = JsonParser::new();
    let mut buf = BytesMut::from(json_str.as_bytes());
    parser.receive(&mut buf, &mut handler).map_err(|e| format!("Invalid JSON: {}", e))?;
    parser.close(&mut handler).map_err(|e| format!("Invalid JSON: {}", e))?;
    Ok(handler.take())
}

pub fn get_profile_dir(base_dir: &str, npub: &str) -> String {
    Path::new(base_dir).join("profiles").join(npub).to_string_lossy().to_string()
}

pub fn ensure_profile_dir(base_dir: &str, npub: &str) -> Result<String, String> {
    let dir = get_profile_dir(base_dir, npub);
    let path = Path::new(&dir);
    if !path.exists() {
        fs::create_dir_all(path).map_err(|e| format!("Could not create profile directory: {}", e))?;
        debug_log!("Created profile directory: {}", dir);
    }
    Ok(dir)
}

pub fn load_app_config(base_dir: &str) -> Result<AppConfig, String> {
    let config_file = Path::new(base_dir).join("plume.json");
    if !config_file.exists() {
        return Ok(AppConfig::new());
    }
    let contents = fs::read_to_string(&config_file)
        .map_err(|e| format!("Could not read plume.json: {}", e))?;
    json_to_app_config(&contents)
}

pub fn save_app_config(base_dir: &str, config: &AppConfig) -> Result<(), String> {
    let config_file = Path::new(base_dir).join("plume.json");
    let json = app_config_to_json(config);
    fs::write(&config_file, json).map_err(|e| format!("Could not write plume.json: {}", e))?;
    debug_log!("Saved app config to: {}", config_file.display());
    Ok(())
}
