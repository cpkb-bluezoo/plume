/*
 * debug.rs
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

//! Debug logging controlled by the `PLUME_DEBUG` environment variable.
//! Set `PLUME_DEBUG=1` to enable verbose WebSocket and relay protocol logging.
//! Defaults to off.

use std::sync::OnceLock;

/// Returns true if verbose debug logging is enabled (`PLUME_DEBUG=1` or `PLUME_DEBUG=true`).
pub fn is_debug() -> bool {
    static DEBUG: OnceLock<bool> = OnceLock::new();
    *DEBUG.get_or_init(|| {
        std::env::var("PLUME_DEBUG")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    })
}

/// Print a message only when `PLUME_DEBUG` is enabled.
/// Usage is identical to `println!`.
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        if $crate::debug::is_debug() {
            println!($($arg)*);
        }
    };
}
