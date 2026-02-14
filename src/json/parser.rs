/*
 * parser.rs
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

//! Push-model JSON parser: feed bytes via `receive()`, get events on a handler.
//! Uses the bytes crate (same buffer contract as jsonparser with Java NIO ByteBuffer).
//!
//! # Buffer management contract
//!
//! The parser consumes only **complete** tokens from the buffer. Incomplete tokens
//! (e.g. a string whose closing `"` has not yet arrived, or a partial number) are
//! **left in the buffer** — the parser advances zero bytes and returns.
//!
//! The caller **must**:
//! - Preserve any unconsumed bytes (do not overwrite or discard them).
//! - Before the next `receive()` call, either compact the buffer so that
//!   unconsumed bytes are at the start, then append new data, or otherwise
//!   re-present the same unconsumed bytes at the front of the buffer.
//!
//! We never consume or throw away unconsumed bytes; the next `receive()` continues
//! from the same position when more data is available.

use bytes::Buf;
use bytes::BytesMut;
use std::collections::VecDeque;

use crate::json::error::JsonError;
use crate::json::handler::JsonContentHandler;
use crate::json::number::JsonNumber;

/// Context for the parser (inside object vs array).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    Object,
    Array,
}

/// Parser state: what we expect next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    Value,   // any value
    Key,     // object key (string) or }
    Colon,   // :
    AfterValue, // comma or ] or }
}

/// Push-model JSON parser. Push bytes with `receive()`; call `close()` at end of input.
pub struct JsonParser {
    /// Skip UTF-8 BOM on first chunk.
    bom_checked: bool,
    closed: bool,
    context_stack: VecDeque<Context>,
    expect: Expect,
    after_comma: bool,
    seen_any_token: bool,
}

impl JsonParser {
    pub fn new() -> Self {
        Self {
            bom_checked: false,
            closed: false,
            context_stack: VecDeque::new(),
            expect: Expect::Value,
            after_comma: false,
            seen_any_token: false,
        }
    }

    /// Push bytes into the parser. Events are delivered to the handler as complete
    /// tokens are recognized. Incomplete tokens are left in the buffer (zero bytes
    /// consumed); the caller must compact and re-present the buffer before the next
    /// `receive()` — see the module-level "Buffer management contract" docs.
    pub fn receive<H: JsonContentHandler>(
        &mut self,
        buf: &mut BytesMut,
        handler: &mut H,
    ) -> Result<(), JsonError> {
        if self.closed {
            return Err(JsonError::new("cannot receive after close"));
        }
        if buf.is_empty() {
            return Ok(());
        }
        if !self.bom_checked {
            // Check for UTF-8 BOM (0xEF 0xBB 0xBF)
            if buf.len() >= 3 && buf[0] == 0xef && buf[1] == 0xbb && buf[2] == 0xbf {
                buf.advance(3);
            } else if buf[0] == 0xef && buf.len() < 3 {
                // Might be a partial BOM; wait for more data
                return Ok(());
            }
            // No BOM (or BOM already skipped) — mark checked and continue to parse
            self.bom_checked = true;
        }
        while !buf.is_empty() {
            let consumed = match self.parse_one(buf, handler)? {
                Some(n) => n,
                // Incomplete token: do not advance; leave all bytes in buffer for next receive().
                None => return Ok(()),
            };
            buf.advance(consumed);
        }
        Ok(())
    }

    /// Signal end of input. Validates that the document is complete.
    pub fn close<H: JsonContentHandler>(&mut self, _handler: &mut H) -> Result<(), JsonError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        if !self.seen_any_token {
            return Err(JsonError::new("no data"));
        }
        if !self.context_stack.is_empty() {
            return Err(JsonError::new("unclosed structure"));
        }
        Ok(())
    }

    /// Reset for parsing a new document.
    #[allow(dead_code)]
    pub fn reset(&mut self) {
        self.bom_checked = false;
        self.closed = false;
        self.context_stack.clear();
        self.expect = Expect::Value;
        self.after_comma = false;
        self.seen_any_token = false;
    }

    /// Parse one token from the front of `data`. Returns bytes consumed, or None if need more data.
    fn parse_one<H: JsonContentHandler>(
        &mut self,
        data: &[u8],
        handler: &mut H,
    ) -> Result<Option<usize>, JsonError> {
        let b = data[0];
        match b {
            b'{' => {
                if self.expect != Expect::Value {
                    return Err(JsonError::new("unexpected '{'"));
                }
                handler.start_object();
                self.context_stack.push_back(Context::Object);
                self.expect = Expect::Key;
                self.after_comma = false;
                self.seen_any_token = true;
                Ok(Some(1))
            }
            b'}' => {
                if self.context_stack.back() != Some(&Context::Object) {
                    return Err(JsonError::new("unexpected '}'"));
                }
                if self.expect != Expect::Key && self.expect != Expect::AfterValue {
                    return Err(JsonError::new("unexpected '}'"));
                }
                if self.after_comma {
                    return Err(JsonError::new("trailing comma before '}'"));
                }
                handler.end_object();
                self.context_stack.pop_back();
                self.expect = Expect::AfterValue;
                self.after_comma = false;
                self.seen_any_token = true;
                Ok(Some(1))
            }
            b'[' => {
                if self.expect != Expect::Value {
                    return Err(JsonError::new("unexpected '['"));
                }
                handler.start_array();
                self.context_stack.push_back(Context::Array);
                self.expect = Expect::Value;
                self.after_comma = false;
                self.seen_any_token = true;
                Ok(Some(1))
            }
            b']' => {
                if self.context_stack.back() != Some(&Context::Array) {
                    return Err(JsonError::new("unexpected ']'"));
                }
                if self.expect != Expect::Value && self.expect != Expect::AfterValue {
                    return Err(JsonError::new("unexpected ']'"));
                }
                if self.after_comma {
                    return Err(JsonError::new("trailing comma before ']'"));
                }
                handler.end_array();
                self.context_stack.pop_back();
                self.expect = Expect::AfterValue;
                self.after_comma = false;
                self.seen_any_token = true;
                Ok(Some(1))
            }
            b',' => {
                if self.expect != Expect::AfterValue {
                    return Err(JsonError::new("unexpected ','"));
                }
                if self.context_stack.is_empty() {
                    return Err(JsonError::new("unexpected comma at root"));
                }
                self.after_comma = true;
                self.expect = if self.context_stack.back() == Some(&Context::Object) {
                    Expect::Key
                } else {
                    Expect::Value
                };
                self.seen_any_token = true;
                Ok(Some(1))
            }
            b':' => {
                if self.expect != Expect::Colon {
                    return Err(JsonError::new("unexpected ':'"));
                }
                self.expect = Expect::Value;
                self.seen_any_token = true;
                Ok(Some(1))
            }
            b' ' | b'\t' | b'\n' | b'\r' => {
                let n = skip_whitespace(data);
                if handler.needs_whitespace() && n > 0 {
                    let ws = std::str::from_utf8(&data[..n])
                        .map_err(|_| JsonError::new("invalid UTF-8 in whitespace"))?;
                    handler.whitespace(ws);
                }
                Ok(Some(n))
            }
            b'"' => {
                if self.expect == Expect::Colon || self.expect == Expect::AfterValue {
                    return Err(JsonError::new("unexpected string"));
                }
                let is_key = self.expect == Expect::Key;
                let (consumed, s) = parse_string(data, self.closed)?;
                if consumed == 0 {
                    // Incomplete string: leave all bytes in buffer for next receive().
                    return Ok(None);
                }
                let s = s.as_str();
                if is_key {
                    handler.key(s);
                    self.expect = Expect::Colon;
                } else {
                    handler.string_value(s);
                    self.expect = Expect::AfterValue;
                }
                self.after_comma = false;
                self.seen_any_token = true;
                Ok(Some(consumed))
            }
            b't' => {
                if self.expect != Expect::Value {
                    return Err(JsonError::new("unexpected 'true'"));
                }
                let n = parse_literal(data, b"rue", self.closed)?;
                if let Some(n) = n {
                    handler.boolean_value(true);
                    self.expect = Expect::AfterValue;
                    self.after_comma = false;
                    self.seen_any_token = true;
                    Ok(Some(n))
                } else {
                    Ok(None) // incomplete literal: leave bytes in buffer
                }
            }
            b'f' => {
                if self.expect != Expect::Value {
                    return Err(JsonError::new("unexpected 'false'"));
                }
                let n = parse_literal(data, b"alse", self.closed)?;
                if let Some(n) = n {
                    handler.boolean_value(false);
                    self.expect = Expect::AfterValue;
                    self.after_comma = false;
                    self.seen_any_token = true;
                    Ok(Some(n))
                } else {
                    Ok(None) // incomplete literal: leave bytes in buffer
                }
            }
            b'n' => {
                if self.expect != Expect::Value {
                    return Err(JsonError::new("unexpected 'null'"));
                }
                let n = parse_literal(data, b"ull", self.closed)?;
                if let Some(n) = n {
                    handler.null_value();
                    self.expect = Expect::AfterValue;
                    self.after_comma = false;
                    self.seen_any_token = true;
                    Ok(Some(n))
                } else {
                    Ok(None) // incomplete literal: leave bytes in buffer
                }
            }
            b'-' | b'0'..=b'9' => {
                if self.expect != Expect::Value {
                    return Err(JsonError::new("unexpected number"));
                }
                let consumed = parse_number(data, self.closed)?;
                if let Some((n, num)) = consumed {
                    handler.number_value(num);
                    self.expect = Expect::AfterValue;
                    self.after_comma = false;
                    self.seen_any_token = true;
                    Ok(Some(n))
                } else {
                    Ok(None) // incomplete number: leave bytes in buffer
                }
            }
            _ => Err(JsonError::new(format!("unexpected character: {}", b as char))),
        }
    }
}

fn skip_whitespace(data: &[u8]) -> usize {
    let mut i = 0;
    while i < data.len() {
        match data[i] {
            b' ' | b'\t' | b'\n' | b'\r' => i += 1,
            _ => break,
        }
    }
    i
}

/// Parse a JSON string starting at data[0] (opening quote). Returns (consumed, unescaped string).
fn parse_string(data: &[u8], closed: bool) -> Result<(usize, String), JsonError> {
    if data.is_empty() || data[0] != b'"' {
        return Err(JsonError::new("expected string"));
    }
    let mut i = 1;
    let mut out = String::new();
    while i < data.len() {
        let b = data[i];
        if b == b'"' {
            i += 1;
            return Ok((i, out));
        }
        if b == b'\\' {
            i += 1;
            if i >= data.len() {
                if closed {
                    return Err(JsonError::new("unclosed string"));
                }
                return Ok((0, String::new())); // need more data; consume nothing
            }
            let (adv, ch) = parse_escape(&data[i..], closed)?;
            if adv == 0 {
                return Ok((0, String::new())); // need more data; consume nothing
            }
            i += adv;
            out.push(ch);
            continue;
        }
        if b < 0x20 {
            return Err(JsonError::new("unescaped control character in string"));
        }
        let (ch, len) = match std::str::from_utf8(&data[i..i + 1]) {
            Ok(s) => (s.chars().next().unwrap(), 1),
            Err(_) => {
                let len = if (data[i] & 0xe0) == 0xc0 {
                    2
                } else if (data[i] & 0xf0) == 0xe0 {
                    3
                } else if (data[i] & 0xf8) == 0xf0 {
                    4
                } else {
                    if !closed {
                        return Ok((0, String::new()));
                    }
                    return Err(JsonError::new("invalid UTF-8 in string"));
                };
                if i + len > data.len() {
                    return Ok((0, String::new())); // need more data for UTF-8; consume nothing
                }
                let s = std::str::from_utf8(&data[i..i + len])
                    .map_err(|_| JsonError::new("invalid UTF-8 in string"))?;
                (s.chars().next().unwrap(), len)
            }
        };
        out.push(ch);
        i += len;
    }
    if closed {
        Err(JsonError::new("unclosed string"))
    } else {
        Ok((0, String::new())) // need more data; consume nothing
    }
}

/// Parse escape sequence after \. Returns (bytes consumed, char). 0 consumed = need more data.
fn parse_escape(data: &[u8], closed: bool) -> Result<(usize, char), JsonError> {
    if data.is_empty() {
        return Ok((0, '\0'));
    }
    let c = data[0];
    Ok(match c {
        b'"' => (1, '"'),
        b'\\' => (1, '\\'),
        b'/' => (1, '/'),
        b'b' => (1, '\u{8}'),
        b'f' => (1, '\u{c}'),
        b'n' => (1, '\n'),
        b'r' => (1, '\r'),
        b't' => (1, '\t'),
        b'u' => {
            if data.len() < 5 {
                if closed {
                    return Err(JsonError::new("incomplete \\u escape"));
                }
                return Ok((0, '\0'));
            }
            let hex = std::str::from_utf8(&data[1..5])
                .map_err(|_| JsonError::new("invalid \\u escape"))?;
            let u = u32::from_str_radix(hex, 16).map_err(|_| JsonError::new("invalid \\u hex"))?;
            let ch = char::from_u32(u).ok_or_else(|| JsonError::new("invalid Unicode code point"))?;
            (5, ch)
        }
        _ => return Err(JsonError::new(format!("invalid escape: \\{}", c as char))),
    })
}

/// Parse literal: first byte already matched (t/f/n), check suffix. Returns Some(consumed) when complete.
fn parse_literal(data: &[u8], suffix: &[u8], closed: bool) -> Result<Option<usize>, JsonError> {
    let need = 1 + suffix.len();
    if data.len() < need {
        if closed {
            return Err(JsonError::new("incomplete literal"));
        }
        return Ok(None);
    }
    for (i, &b) in suffix.iter().enumerate() {
        if data[1 + i] != b {
            return Err(JsonError::new("invalid literal"));
        }
    }
    Ok(Some(need))
}

/// Parse number. Returns Some((consumed, JsonNumber)) when complete.
fn parse_number(data: &[u8], closed: bool) -> Result<Option<(usize, JsonNumber)>, JsonError> {
    if data.is_empty() {
        return Ok(None);
    }
    let mut i = 0;
    if data[i] == b'-' {
        i += 1;
        if i >= data.len() {
            return Ok(None);
        }
    }
    if data[i] == b'0' {
        i += 1;
        if i < data.len() && data[i] >= b'0' && data[i] <= b'9' {
            return Err(JsonError::new("numbers cannot have leading zeros"));
        }
    } else if data[i] >= b'1' && data[i] <= b'9' {
        while i < data.len() && data[i] >= b'0' && data[i] <= b'9' {
            i += 1;
        }
    } else {
        return Err(JsonError::new("invalid number"));
    }
    let has_dot = i < data.len() && data[i] == b'.';
    if has_dot {
        i += 1;
        if i >= data.len() {
            return Ok(None);
        }
        if data[i] < b'0' || data[i] > b'9' {
            return Err(JsonError::new("decimal point must be followed by digit"));
        }
        while i < data.len() && data[i] >= b'0' && data[i] <= b'9' {
            i += 1;
        }
    }
    let has_e = i < data.len() && (data[i] == b'e' || data[i] == b'E');
    if has_e {
        i += 1;
        if i >= data.len() {
            return Ok(None);
        }
        if data[i] == b'+' || data[i] == b'-' {
            i += 1;
        }
        if i >= data.len() || data[i] < b'0' || data[i] > b'9' {
            return Err(JsonError::new("exponent must have digit"));
        }
        while i < data.len() && data[i] >= b'0' && data[i] <= b'9' {
            i += 1;
        }
    }
    if i < data.len() {
        let c = data[i];
        if c == b'.' || c == b'e' || c == b'E' || (c >= b'0' && c <= b'9') {
            return Ok(None);
        }
    } else if !closed {
        return Ok(None);
    }
    let s = std::str::from_utf8(&data[..i]).map_err(|_| JsonError::new("invalid UTF-8 in number"))?;
    let num = if s.contains('.') || s.contains('e') || s.contains('E') {
        let f: f64 = s.parse().map_err(|e| JsonError::with_source("invalid number", e))?;
        JsonNumber::F64(f)
    } else {
        let i64_val: i64 = s.parse().map_err(|e| JsonError::with_source("invalid number", e))?;
        JsonNumber::I64(i64_val)
    };
    Ok(Some((i, num)))
}
