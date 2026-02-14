/*
 * handshake.rs
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

//! WebSocket opening handshake (RFC 6455 §4): GET with Upgrade, parse 101, verify Sec-WebSocket-Accept.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::io;

/// Magic string for Sec-WebSocket-Accept (RFC 6455 §4.2.2).
const WS_ACCEPT_MAGIC: &[u8] = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Build the HTTP GET request for the WebSocket handshake. Caller writes this to the stream.
pub fn build_handshake_request(host: &str, port: u16, path: &str, key_base64: &str) -> Vec<u8> {
    let host_header = if port == 80 || port == 443 {
        host.to_string()
    } else {
        format!("{}:{}", host, port)
    };
    let mut req = Vec::new();
    req.extend_from_slice(b"GET ");
    req.extend_from_slice(path.as_bytes());
    req.extend_from_slice(b" HTTP/1.1\r\nHost: ");
    req.extend_from_slice(host_header.as_bytes());
    req.extend_from_slice(b"\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: ");
    req.extend_from_slice(key_base64.as_bytes());
    req.extend_from_slice(b"\r\nSec-WebSocket-Version: 13\r\n\r\n");
    req
}

/// Compute expected Sec-WebSocket-Accept from the base64-encoded key we sent.
pub fn compute_expected_accept(key_base64: &str) -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(key_base64.as_bytes());
    hasher.update(WS_ACCEPT_MAGIC);
    let digest = hasher.finalize();
    BASE64.encode(digest.as_slice())
}

/// Verify the server's Sec-WebSocket-Accept header matches our key.
pub fn verify_accept(accept_header: Option<&str>, key_base64: &str) -> Result<(), io::Error> {
    let expected = compute_expected_accept(key_base64);
    match accept_header {
        Some(h) if h.trim() == expected => Ok(()),
        Some(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Sec-WebSocket-Accept mismatch",
        )),
        None => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "missing Sec-WebSocket-Accept",
        )),
    }
}

/// Minimal 101-response parser result.
pub struct HandshakeResponse {
    pub status: u16,
    pub accept: Option<String>,
    /// Byte offset where the HTTP body (WebSocket frames) begins in the input buffer.
    pub body_offset: usize,
}

/// Parse the 101 Switching Protocols response from a buffer.
/// Looks for the status line, extracts headers, stops at the empty CRLF line.
/// Returns None if the response is not yet complete (need more data).
/// On success, `body_offset` indicates where WebSocket frame data starts in `buf`.
pub fn parse_101_response(buf: &[u8]) -> Option<io::Result<HandshakeResponse>> {
    // Find the end of headers: \r\n\r\n
    let crlf2_pos = find_header_end(buf)?;
    let body_offset = crlf2_pos + 4; // skip past \r\n\r\n
    let header_bytes = &buf[..crlf2_pos];
    let header_str = match std::str::from_utf8(header_bytes) {
        Ok(s) => s,
        Err(_) => return Some(Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid UTF-8 in HTTP response",
        ))),
    };

    let mut lines = header_str.split("\r\n");

    // Status line: HTTP/1.1 101 Switching Protocols
    let status_line = match lines.next() {
        Some(l) => l,
        None => return Some(Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "empty HTTP response",
        ))),
    };
    let parts: Vec<&str> = status_line.splitn(3, ' ').collect();
    let status: u16 = parts
        .get(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    // Headers
    let mut accept: Option<String> = None;
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some(colon) = line.find(':') {
            let name = line[..colon].trim();
            let value = line[colon + 1..].trim();
            if name.eq_ignore_ascii_case("Sec-WebSocket-Accept") {
                accept = Some(value.to_string());
            }
        }
    }

    Some(Ok(HandshakeResponse { status, accept, body_offset }))
}

/// Find \r\n\r\n in buffer. Returns the offset of the first \r in \r\n\r\n.
fn find_header_end(buf: &[u8]) -> Option<usize> {
    if buf.len() < 4 {
        return None;
    }
    for i in 0..buf.len() - 3 {
        if buf[i] == b'\r' && buf[i + 1] == b'\n' && buf[i + 2] == b'\r' && buf[i + 3] == b'\n' {
            return Some(i);
        }
    }
    None
}
