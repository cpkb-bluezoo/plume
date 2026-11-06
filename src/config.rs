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

/// Relays to use when a profile has none configured yet (a brand new
/// identity, or one whose relay list hasn't loaded/been fetched). Never
/// written into a saved [`Config`] -- see [`Config::new`]'s comment --
/// callers needing a relay list fall back to this themselves when
/// `cfg.relays` is empty, same as the frontend's `getEffectiveRelays`.
///
/// Onion addresses: a Tor-routed client reaches these through Tor's own
/// hidden-service rendezvous, never a public exit node, so they're
/// unaffected by relays that block or rate-limit Tor exit traffic
/// (confirmed live: nos.lol and relay.nostr.band, this list's previous
/// contents, both refuse connections from Tor exits while answering
/// clearnet clients instantly). Each lives on different operator
/// infrastructure so one operator's downtime doesn't take out the whole
/// bootstrap list.
pub const BOOTSTRAP_RELAYS: [&str; 3] = [
    "ws://oxtrdevav64z64yb7x6rjg4ntzqjhedm5b5zjqulugknhzr46ny2qbad.onion", // nostr.oxtr.dev
    "ws://gnostr2jnapk72mnagq3cuykfon73temzp77hcbncn4silgt77boruid.onion", // nostr.girino.org
    "ws://35vr3xigzjv2xyzfyif6o2gksmkioppy4rmwag7d4bqmwuccs2u4jaid.onion", // Noderunners relay
];

/// Where `Config::private_key` lives at rest. `Plaintext` (the default,
/// and the only option when no OS keyring is available) keeps it inline
/// in this profile's `config.json`, exactly as before this existed.
/// `Keyring` means the secret lives in the OS-native secret store instead
/// (see `keyring_store.rs`) and is never written to `config.json` -- only
/// opted into explicitly, never migrated to automatically.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyStorage {
    Plaintext,
    Keyring,
}

impl KeyStorage {
    pub fn as_str(&self) -> &'static str {
        match self {
            KeyStorage::Plaintext => "plaintext",
            KeyStorage::Keyring => "keyring",
        }
    }

    pub fn from_str(value: &str) -> KeyStorage {
        match value {
            "keyring" => KeyStorage::Keyring,
            _ => KeyStorage::Plaintext,
        }
    }
}

// The main configuration structure.
// Profile fields (name, about, picture, nip05, banner, website, lud16) are stored
// directly rather than embedded as a JSON string, matching the Nostr kind 0 field names.
pub struct Config {
    pub public_key: String,
    pub private_key: Option<String>,
    pub key_storage: KeyStorage,
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
    /// NIP-17: Preferred relays for receiving private messages (kind 10050).
    pub dm_relays: Vec<String>,
    /// created_at of the most recent DM event (kind 4 or 1059) received from relays.
    /// Used for incremental sync with a 2-day safety margin.
    pub dm_sync_since: u64,
}

impl Config {
    pub fn new() -> Config {
        Config {
            public_key: String::new(),
            private_key: None,
            key_storage: KeyStorage::Plaintext,
            // Deliberately empty, not BOOTSTRAP_RELAYS -- a brand new profile
            // has no opinion of its own about relays yet, so there's nothing
            // to save here. Callers that need a relay list and find this
            // empty fall back to BOOTSTRAP_RELAYS themselves (see its
            // comment) without writing it back into the config: once the
            // profile is fetched from the network, or the user adds/changes
            // relays in Settings, that real list is what gets saved, never
            // a bootstrap default that happened to be in effect at the time.
            relays: Vec::new(),
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
            dm_relays: Vec::new(),
            dm_sync_since: 0,
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
    DmRelays,
}

struct ConfigHandler {
    depth: i32,
    current_field: Option<String>,
    array_field: ConfigArrayField,
    // Scalar fields
    public_key: String,
    private_key: Option<String>,
    key_storage: KeyStorage,
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
    dm_sync_since: u64,
    // Array fields
    relays: Vec<String>,
    dm_relays: Vec<String>,
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
            key_storage: KeyStorage::Plaintext,
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
            dm_sync_since: 0,
            relays: Vec::new(),
            dm_relays: Vec::new(),
            following: Vec::new(),
            muted_users: Vec::new(),
            muted_words: Vec::new(),
            muted_hashtags: Vec::new(),
            bookmarks: Vec::new(),
            profile_metadata_raw: None,
        }
    }

    fn take_config(mut self) -> Config {
        // Deliberately left as whatever was in the file -- see Config::new()'s
        // comment on why an empty relay list is never auto-filled in here.
        let relays = self.relays;
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
            key_storage: self.key_storage,
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
            dm_relays: self.dm_relays,
            dm_sync_since: self.dm_sync_since,
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
                    "dm_relays" => ConfigArrayField::DmRelays,
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
                ConfigArrayField::DmRelays => &mut self.dm_relays,
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
                    "key_storage" => self.key_storage = KeyStorage::from_str(value),
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
                } else if f == "dm_sync_since" {
                    self.dm_sync_since = number.as_f64().max(0.0) as u64;
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
    // Never write the secret to disk (or hand it back over IPC) when it
    // lives in the OS keyring instead -- regardless of whether it's
    // currently resolved in memory. This is the one line that guarantees
    // the "shouldn't be present in the config file" requirement.
    match (&config.private_key, config.key_storage) {
        (Some(key), KeyStorage::Plaintext) => {
            json.push_str("\"");
            json.push_str(&escape_json_string(key));
            json.push_str("\"");
        }
        _ => json.push_str("null"),
    }
    json.push_str(",\n");

    json.push_str("  \"key_storage\": \"");
    json.push_str(config.key_storage.as_str());
    json.push_str("\",\n");

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
    json.push_str(",\n");

    write_string_array(&mut json, "dm_relays", &config.dm_relays);
    json.push_str(",\n");

    json.push_str("  \"dm_sync_since\": ");
    json.push_str(&config.dm_sync_since.to_string());
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

/// `cfg.relays` if it has any, else [`BOOTSTRAP_RELAYS`] -- the Rust-side
/// equivalent of the frontend's `getEffectiveRelays`. Every caller that
/// fetches or publishes using a profile's relays should go through this
/// rather than reading `cfg.relays` directly, so a brand new profile (or
/// one whose relay list hasn't been learned yet) still has somewhere to
/// talk to instead of failing outright.
pub fn effective_relays(cfg: &Config) -> Vec<String> {
    if cfg.relays.is_empty() {
        BOOTSTRAP_RELAYS.iter().map(|s| s.to_string()).collect()
    } else {
        cfg.relays.clone()
    }
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
    let mut config = json_to_config(&contents)?;
    if config.key_storage == KeyStorage::Keyring {
        // Swallow lookup errors into `None` rather than failing the whole
        // load: every caller of `private_key` already treats `None` as
        // "no private key configured" with its own clear error message,
        // so there's no new error path to thread through every call site.
        config.private_key = crate::keyring_store::load(&config.public_key).unwrap_or(None);
    }
    Ok(config)
}

pub fn save_config(config_dir: &str, config: &Config) -> Result<(), String> {
    if config.key_storage == KeyStorage::Keyring {
        if let Some(ref key) = config.private_key {
            crate::keyring_store::store(&config.public_key, key)?;
        }
    }
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

#[cfg(test)]
mod key_storage_tests {
    use super::*;

    #[test]
    fn key_storage_defaults_to_plaintext_when_absent_from_an_old_config_file() {
        let json = "{\"public_key\": \"abc\", \"private_key\": \"def\"}";
        let cfg = json_to_config(json).expect("should parse");
        assert_eq!(cfg.key_storage, KeyStorage::Plaintext);
        assert_eq!(cfg.private_key, Some(String::from("def")));
    }

    #[test]
    fn plaintext_private_key_round_trips_through_json() {
        let mut cfg = Config::new();
        cfg.public_key = String::from("abc");
        cfg.private_key = Some(String::from("secret-hex"));
        cfg.key_storage = KeyStorage::Plaintext;

        let json = config_to_json(&cfg);
        assert!(json.contains("\"private_key\": \"secret-hex\""));
        assert!(json.contains("\"key_storage\": \"plaintext\""));

        let round_tripped = json_to_config(&json).expect("should parse");
        assert_eq!(round_tripped.private_key, Some(String::from("secret-hex")));
        assert_eq!(round_tripped.key_storage, KeyStorage::Plaintext);
    }

    #[test]
    fn keyring_mode_never_writes_the_secret_to_json_even_if_resolved_in_memory() {
        let mut cfg = Config::new();
        cfg.public_key = String::from("abc");
        // Simulates a Config whose private_key was just resolved from the
        // keyring for in-memory use (e.g. by a signing command) -- it must
        // never be serialized back out, on disk or over IPC.
        cfg.private_key = Some(String::from("super-secret-hex"));
        cfg.key_storage = KeyStorage::Keyring;

        let json = config_to_json(&cfg);
        assert!(json.contains("\"private_key\": null"));
        assert!(json.contains("\"key_storage\": \"keyring\""));
        assert!(!json.contains("super-secret-hex"));
    }

    #[test]
    fn keyring_mode_flag_round_trips_but_private_key_comes_back_none() {
        let mut cfg = Config::new();
        cfg.public_key = String::from("abc");
        cfg.private_key = Some(String::from("super-secret-hex"));
        cfg.key_storage = KeyStorage::Keyring;
        let json = config_to_json(&cfg);

        // json_to_config alone (no keyring lookup -- that's load_config's
        // job) should come back with key_storage preserved and no secret.
        let round_tripped = json_to_config(&json).expect("should parse");
        assert_eq!(round_tripped.key_storage, KeyStorage::Keyring);
        assert_eq!(round_tripped.private_key, None);
    }

    #[test]
    fn load_config_resolves_a_keyring_stored_key_and_save_config_never_writes_it_to_disk() {
        if !crate::keyring_store::is_available() {
            eprintln!("skipping: no usable OS keyring backend on this machine");
            return;
        }
        let dir = std::env::temp_dir().join(format!(
            "plume_config_keyring_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).expect("should create temp dir");
        let dir_str = dir.to_string_lossy().to_string();

        let mut cfg = Config::new();
        cfg.public_key = String::from("plume_test_config_keyring_integration_pubkey");
        cfg.private_key = Some(String::from("integration-test-secret-hex"));
        cfg.key_storage = KeyStorage::Keyring;
        save_config(&dir_str, &cfg).expect("save_config should succeed");

        let on_disk = fs::read_to_string(dir.join("config.json")).expect("config.json should exist");
        assert!(on_disk.contains("\"private_key\": null"));
        assert!(!on_disk.contains("integration-test-secret-hex"));

        let loaded = load_config(&dir_str).expect("load_config should succeed");
        assert_eq!(loaded.private_key, Some(String::from("integration-test-secret-hex")));
        assert_eq!(loaded.key_storage, KeyStorage::Keyring);

        let _ = crate::keyring_store::delete(&cfg.public_key);
        let _ = fs::remove_dir_all(&dir);
    }
}
