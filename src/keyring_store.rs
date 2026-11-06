/*
 * keyring_store.rs
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

//! Thin, mechanical wrapper around the OS-native secret store (macOS
//! Keychain, Windows Credential Manager, Linux Secret Service via D-Bus).
//! Holds no policy about *when* a private key should live here instead of
//! in plaintext config -- that decision belongs to `config.rs`/`main.rs`.
//! Entries are keyed by the account's hex public key.

use std::sync::OnceLock;

use keyring::Entry;

const SERVICE_NAME: &str = "Plume";
const PROBE_ACCOUNT: &str = "__plume_availability_probe__";

/// Whether a usable OS secret store exists on this machine. macOS and
/// Windows are effectively always available; Linux commonly isn't (no
/// Secret Service daemon running, e.g. headless or over SSH), so this is a
/// runtime probe rather than a platform allowlist. Cached for the life of
/// the process -- callers should not expect this to change mid-session.
pub fn is_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(probe)
}

fn probe() -> bool {
    let entry = match Entry::new(SERVICE_NAME, PROBE_ACCOUNT) {
        Ok(e) => e,
        Err(_) => return false,
    };
    if entry.set_password("probe").is_err() {
        return false;
    }
    let readable = entry.get_password().is_ok();
    let _ = entry.delete_credential();
    readable
}

pub fn store(account_hex_pubkey: &str, secret_hex: &str) -> Result<(), String> {
    let entry = Entry::new(SERVICE_NAME, account_hex_pubkey)
        .map_err(|e| format!("Could not access system keyring: {}", e))?;
    entry
        .set_password(secret_hex)
        .map_err(|e| format!("Could not store private key in system keyring: {}", e))
}

pub fn load(account_hex_pubkey: &str) -> Result<Option<String>, String> {
    let entry = Entry::new(SERVICE_NAME, account_hex_pubkey)
        .map_err(|e| format!("Could not access system keyring: {}", e))?;
    match entry.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Could not read private key from system keyring: {}", e)),
    }
}

pub fn delete(account_hex_pubkey: &str) -> Result<(), String> {
    let entry = Entry::new(SERVICE_NAME, account_hex_pubkey)
        .map_err(|e| format!("Could not access system keyring: {}", e))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("Could not remove private key from system keyring: {}", e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These exercise the real OS secret store, so they skip cleanly
    /// (rather than failing) on a machine with no usable backend -- e.g.
    /// CI or a headless Linux box with no Secret Service daemon running.
    fn require_backend() -> bool {
        if !is_available() {
            eprintln!("skipping: no usable OS keyring backend on this machine");
            return false;
        }
        true
    }

    #[test]
    fn store_then_load_round_trips_a_secret() {
        if !require_backend() {
            return;
        }
        let account = "plume_test_store_then_load";
        store(account, "deadbeef").expect("store should succeed");
        let loaded = load(account).expect("load should succeed");
        assert_eq!(loaded, Some(String::from("deadbeef")));
        delete(account).expect("delete should succeed");
    }

    #[test]
    fn load_of_an_absent_account_is_ok_none_not_an_error() {
        if !require_backend() {
            return;
        }
        let loaded = load("plume_test_account_that_was_never_stored").expect("load should succeed");
        assert_eq!(loaded, None);
    }

    #[test]
    fn delete_of_an_absent_account_is_tolerated() {
        if !require_backend() {
            return;
        }
        delete("plume_test_account_that_was_never_stored").expect("delete of a missing entry should not error");
    }
}
