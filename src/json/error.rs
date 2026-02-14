/*
 * error.rs
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

//! JSON parse/serialize errors.

use std::fmt;

/// Error during JSON parsing or writing.
#[derive(Debug)]
pub struct JsonError {
    message: String,
    #[allow(dead_code)]
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl JsonError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            message: msg.into(),
            source: None,
        }
    }

    pub fn with_source(msg: impl Into<String>, source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
        Self {
            message: msg.into(),
            source: Some(source.into()),
        }
    }
}

impl fmt::Display for JsonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for JsonError {}
