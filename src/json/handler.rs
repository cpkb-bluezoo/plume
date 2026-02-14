/*
 * handler.rs
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

//! Content handler for JSON parse events (push model).

use crate::json::number::JsonNumber;

/// Handler for JSON parsing events. The parser calls these methods as tokens are recognized.
/// String/key data is valid only for the duration of the call.
pub trait JsonContentHandler {
    fn start_object(&mut self);
    fn end_object(&mut self);
    fn start_array(&mut self);
    fn end_array(&mut self);
    fn number_value(&mut self, number: JsonNumber);
    fn string_value(&mut self, value: &str);
    fn boolean_value(&mut self, value: bool);
    fn null_value(&mut self);
    /// Whitespace between tokens. Not reported unless `needs_whitespace()` is true.
    fn whitespace(&mut self, _ws: &str) {}
    /// Key (property name) in an object; always follows start_object or a previous value.
    fn key(&mut self, key: &str);

    /// If true, the parser will call `whitespace()` for whitespace sequences. Default false.
    fn needs_whitespace(&self) -> bool {
        false
    }
}
