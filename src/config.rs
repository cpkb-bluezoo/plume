// Plume - Configuration Management
// Handles reading and writing configuration from ~/.plume

use std::fs;
use std::io;
use std::path::Path;

// The main configuration structure
// This gets saved to and loaded from ~/.plume/config.json
pub struct Config {
    // User's Nostr public key (npub or hex)
    pub public_key: String,
    
    // User's Nostr private key (nsec or hex) - optional, for signing
    pub private_key: Option<String>,
    
    // List of relay URLs to connect to
    pub relays: Vec<String>,
    
    // Display name for the user
    pub display_name: String,
}

// Create a default configuration for new users
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
        }
    }
}

// Convert a Config struct to a JSON string
// We do this manually so you can see exactly what's happening
pub fn config_to_json(config: &Config) -> String {
    // Start building the JSON object
    let mut json = String::new();
    json.push_str("{\n");
    
    // Add public_key field
    json.push_str("  \"public_key\": \"");
    json.push_str(&escape_json_string(&config.public_key));
    json.push_str("\",\n");
    
    // Add private_key field (can be null)
    json.push_str("  \"private_key\": ");
    match &config.private_key {
        Some(key) => {
            json.push_str("\"");
            json.push_str(&escape_json_string(key));
            json.push_str("\"");
        }
        None => {
            json.push_str("null");
        }
    }
    json.push_str(",\n");
    
    // Add relays array
    json.push_str("  \"relays\": [\n");
    for (index, relay) in config.relays.iter().enumerate() {
        json.push_str("    \"");
        json.push_str(&escape_json_string(relay));
        json.push_str("\"");
        // Add comma if not the last item
        if index < config.relays.len() - 1 {
            json.push_str(",");
        }
        json.push_str("\n");
    }
    json.push_str("  ],\n");
    
    // Add display_name field
    json.push_str("  \"display_name\": \"");
    json.push_str(&escape_json_string(&config.display_name));
    json.push_str("\"\n");
    
    json.push_str("}");
    
    return json;
}

// Parse a JSON string into a Config struct
// We do this manually so you can see exactly what's happening
pub fn json_to_config(json_str: &str) -> Result<Config, String> {
    // Use the json crate to parse the string
    let parsed = match json::parse(json_str) {
        Ok(value) => value,
        Err(e) => return Err(format!("Invalid JSON: {}", e)),
    };
    
    // Extract public_key (required string field)
    let public_key: String;
    if parsed["public_key"].is_string() {
        public_key = parsed["public_key"].as_str().unwrap().to_string();
    } else {
        public_key = String::new();
    }
    
    // Extract private_key (optional string field)
    let private_key: Option<String>;
    if parsed["private_key"].is_string() {
        private_key = Some(parsed["private_key"].as_str().unwrap().to_string());
    } else {
        private_key = None;
    }
    
    // Extract display_name (string with default)
    let display_name: String;
    if parsed["display_name"].is_string() {
        display_name = parsed["display_name"].as_str().unwrap().to_string();
    } else {
        display_name = String::from("Anonymous");
    }
    
    // Extract relays (array of strings)
    let mut relays: Vec<String> = Vec::new();
    if parsed["relays"].is_array() {
        for item in parsed["relays"].members() {
            if item.is_string() {
                relays.push(item.as_str().unwrap().to_string());
            }
        }
    }
    // Use defaults if no relays specified
    if relays.is_empty() {
        relays.push(String::from("wss://relay.damus.io"));
        relays.push(String::from("wss://nos.lol"));
        relays.push(String::from("wss://relay.nostr.band"));
    }
    
    // Build and return the Config struct
    let config = Config {
        public_key: public_key,
        private_key: private_key,
        relays: relays,
        display_name: display_name,
    };
    
    return Ok(config);
}

// Escape special characters in a string for JSON
// This handles quotes, backslashes, newlines, etc.
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

// Get the path to the config directory (~/.plume)
pub fn get_config_dir() -> Option<String> {
    // Use the dirs crate to find the home directory
    match dirs::home_dir() {
        Some(home) => {
            let config_path = home.join(".plume");
            // Convert the path to a string
            match config_path.to_str() {
                Some(s) => return Some(String::from(s)),
                None => return None,
            }
        }
        None => {
            return None;
        }
    }
}

// Make sure the config directory exists, create it if not
pub fn ensure_config_dir(config_dir: &str) -> Result<(), io::Error> {
    let path = Path::new(config_dir);
    
    if path.exists() {
        // Already exists, nothing to do
        return Ok(());
    }
    
    // Create the directory
    fs::create_dir_all(path)?;
    
    println!("Created config directory: {}", config_dir);
    return Ok(());
}

// Get the full path to the config file
fn get_config_file_path(config_dir: &str) -> String {
    let path = Path::new(config_dir).join("config.json");
    return path.to_string_lossy().to_string();
}

// Load configuration from disk
pub fn load_config(config_dir: &str) -> Result<Config, String> {
    let config_file = get_config_file_path(config_dir);
    let path = Path::new(&config_file);
    
    // If config file doesn't exist, return default config
    if !path.exists() {
        println!("No config file found, using defaults");
        return Ok(Config::new());
    }
    
    // Read the file contents
    let contents = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => return Err(format!("Could not read config file: {}", e)),
    };
    
    // Parse the JSON into a Config struct
    return json_to_config(&contents);
}

// Save configuration to disk
pub fn save_config(config_dir: &str, config: &Config) -> Result<(), String> {
    let config_file = get_config_file_path(config_dir);
    
    // Convert config to JSON string
    let json = config_to_json(config);
    
    // Write to file
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
