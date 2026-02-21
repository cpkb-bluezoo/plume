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

//! HTTP/1.1 response push parser: status line, headers, body (Content-Length or chunked).

use bytes::Buf;
use bytes::BytesMut;
use std::io;

/// Callback for HTTP/1.1 response events. Connection implements this and
/// forwards to ResponseHandler.
pub trait H1ResponseHandler {
    fn status(&mut self, code: u16, reason: Option<&str>);
    fn header(&mut self, name: &str, value: &str);
    fn body_chunk(&mut self, data: &[u8]);
    fn end_body(&mut self);
    fn trailer(&mut self, name: &str, value: &str);
    fn complete(&mut self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseState {
    Idle,
    StatusLine,
    Headers,
    /// Headers done; connection must call set_body_mode() and optionally
    /// handler.start_body().
    HeadersComplete,
    Body,
    ChunkSize,
    ChunkData,
    ChunkTrailer,
}

/// Push parser for HTTP/1.1 response. Feed bytes via `receive`; handler is
/// invoked as complete tokens are parsed.
pub struct ResponseParser {
    state: ParseState,
    content_length: i64,
    bytes_received: i64,
    chunk_remaining: i64,
}

impl ResponseParser {
    pub fn new() -> Self {
        Self {
            state: ParseState::StatusLine,
            content_length: -1,
            bytes_received: 0,
            chunk_remaining: 0,
        }
    }

    pub fn state(&self) -> ParseState {
        self.state
    }

    pub fn reset(&mut self) {
        self.state = ParseState::StatusLine;
        self.content_length = -1;
        self.bytes_received = 0;
        self.chunk_remaining = 0;
    }

    fn find_crlf(buf: &[u8]) -> Option<usize> {
        let mut i = 0;
        while i + 1 < buf.len() {
            if buf[i] == b'\r' && buf[i + 1] == b'\n' {
                return Some(i);
            }
            i += 1;
        }
        None
    }

    /// Consume and parse as much as possible from buf. Handler is called for
    /// each complete token. Partial data remains in buf.
    pub fn receive<H: H1ResponseHandler>(
        &mut self,
        buf: &mut BytesMut,
        handler: &mut H,
    ) -> Result<(), io::Error> {
        while !buf.is_empty() {
            match self.state {
                ParseState::StatusLine => {
                    let line_end = match Self::find_crlf(buf) {
                        Some(n) => n,
                        None => return Ok(()),
                    };
                    let line = buf.split_to(line_end + 2);
                    let line_str = std::str::from_utf8(&line[..line_end]).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "invalid status line UTF-8")
                    })?;
                    let parts: Vec<&str> = line_str.splitn(3, ' ').collect();
                    let code = parts
                        .get(1)
                        .and_then(|s| s.parse::<u16>().ok())
                        .unwrap_or(0);
                    let reason = parts.get(2).copied();
                    handler.status(code, reason);
                    self.state = ParseState::Headers;
                }
                ParseState::Headers => {
                    let line_end = match Self::find_crlf(buf) {
                        Some(n) => n,
                        None => return Ok(()),
                    };
                    if line_end == 0 {
                        buf.advance(2);
                        self.state = ParseState::HeadersComplete;
                        return Ok(());
                    }
                    let line = buf.split_to(line_end + 2);
                    let line_str = std::str::from_utf8(&line[..line_end]).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidData, "invalid header UTF-8")
                    })?;
                    if let Some(colon) = line_str.find(':') {
                        let name = line_str[..colon].trim();
                        let value = line_str[colon + 1..].trim();
                        handler.header(name, value);
                    }
                }
                ParseState::Body => {
                    if self.content_length >= 0 {
                        let remaining = (self.content_length - self.bytes_received) as usize;
                        let to_read = remaining.min(buf.len());
                        if to_read > 0 {
                            let chunk = buf.split_to(to_read);
                            handler.body_chunk(&chunk);
                            self.bytes_received += to_read as i64;
                        }
                        if self.bytes_received >= self.content_length {
                            handler.end_body();
                            handler.complete();
                            self.state = ParseState::Idle;
                        }
                    } else {
                        if !buf.is_empty() {
                            let chunk = buf.split_to(buf.len());
                            handler.body_chunk(&chunk);
                        }
                        return Ok(());
                    }
                }
                ParseState::ChunkSize => {
                    let line_end = match Self::find_crlf(buf) {
                        Some(n) => n,
                        None => return Ok(()),
                    };
                    let line = buf.split_to(line_end + 2);
                    let line_str =
                        std::str::from_utf8(&line[..line_end]).map_err(|_| {
                            io::Error::new(io::ErrorKind::InvalidData, "invalid chunk size")
                        })?;
                    let hex_part = line_str.split(';').next().unwrap_or(line_str).trim();
                    self.chunk_remaining =
                        i64::from_str_radix(hex_part, 16).unwrap_or(0);
                    if self.chunk_remaining == 0 {
                        self.state = ParseState::ChunkTrailer;
                    } else {
                        self.state = ParseState::ChunkData;
                    }
                }
                ParseState::ChunkData => {
                    let to_read = (self.chunk_remaining as usize).min(buf.len());
                    if to_read > 0 {
                        let chunk = buf.split_to(to_read);
                        handler.body_chunk(&chunk);
                        self.chunk_remaining -= to_read as i64;
                    }
                    if self.chunk_remaining == 0 {
                        if buf.len() >= 2 {
                            buf.advance(2);
                            self.state = ParseState::ChunkSize;
                        } else {
                            return Ok(());
                        }
                    } else {
                        return Ok(());
                    }
                }
                ParseState::HeadersComplete => {
                    return Ok(());
                }
                ParseState::ChunkTrailer => {
                    let line_end = match Self::find_crlf(buf) {
                        Some(n) => n,
                        None => return Ok(()),
                    };
                    if line_end == 0 {
                        buf.advance(2);
                        handler.end_body();
                        handler.complete();
                        self.state = ParseState::Idle;
                    } else {
                        let line = buf.split_to(line_end + 2);
                        let line_str =
                            std::str::from_utf8(&line[..line_end]).map_err(|_| {
                                io::Error::new(io::ErrorKind::InvalidData, "invalid trailer")
                            })?;
                        if let Some(colon) = line_str.find(':') {
                            let name = line_str[..colon].trim();
                            let value = line_str[colon + 1..].trim();
                            handler.trailer(name, value);
                        }
                    }
                }
                ParseState::Idle => return Ok(()),
            }
        }
        Ok(())
    }

    /// Called by the connection after headers are received (state
    /// HeadersComplete). Connection should call handler.start_body() first
    /// if body is expected.
    pub fn set_body_mode(&mut self, content_length: Option<u64>, chunked: bool) {
        if self.state != ParseState::HeadersComplete {
            return;
        }
        if chunked {
            self.content_length = -1;
            self.state = ParseState::ChunkSize;
        } else if let Some(cl) = content_length {
            self.content_length = cl as i64;
            self.bytes_received = 0;
            if cl == 0 {
                self.state = ParseState::Idle;
            } else {
                self.state = ParseState::Body;
            }
        } else {
            self.content_length = -1;
            self.state = ParseState::Body;
        }
    }
}

impl Default for ResponseParser {
    fn default() -> Self {
        Self::new()
    }
}
