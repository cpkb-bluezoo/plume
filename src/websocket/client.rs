/*
 * client.rs
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

//! WebSocket client: connect to ws:// or wss:// URL, perform handshake, return WebSocketConnection.

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use url::Url;

use crate::websocket::connection::WebSocketConnection;
use crate::websocket::handshake::{
    build_handshake_request, parse_101_response, verify_accept,
};
use crate::websocket::stream::{connect_tls, WsStream};

/// WebSocket client. Connect with `WebSocketClient::connect(url)`.
pub struct WebSocketClient;

impl WebSocketClient {
    /// Connect to the given WebSocket URL (ws:// or wss://), perform the opening handshake,
    /// and return a `WebSocketConnection`. Call `connected()` on your handler, then use
    /// `conn.run(handler)` to drive the read loop and `conn.send_text()` etc. to send.
    pub async fn connect(url: &str) -> io::Result<WebSocketConnection> {
        let url = Url::parse(url).map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidInput, e.to_string())
        })?;
        let host = url.host_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "URL has no host")
        })?;
        let port = url.port_or_known_default().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "URL has no port")
        })?;
        let path = if url.path().is_empty() {
            "/"
        } else {
            url.path()
        };
        let use_tls = matches!(url.scheme(), "wss" | "https");
        if !matches!(url.scheme(), "ws" | "wss") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "URL scheme must be ws or wss",
            ));
        }

        let addr = format!("{}:{}", host, port);
        let tcp = TcpStream::connect(&addr).await?;

        let mut stream = if use_tls {
            connect_tls(tcp, host).await?
        } else {
            WsStream::Plain(tcp)
        };

        // Handshake: 16 random bytes -> base64 key
        let mut key_raw = [0u8; 16];
        rand::RngCore::fill_bytes(&mut rand::thread_rng(), &mut key_raw);
        let key_base64 = BASE64.encode(&key_raw);

        let request = build_handshake_request(host, port, path, &key_base64);
        stream.write_all(&request).await?;
        stream.flush().await?;

        let mut read_buf = Vec::with_capacity(4096);
        let body_offset: usize;
        loop {
            let mut tmp = [0u8; 4096];
            let n = stream.read(&mut tmp).await?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection closed during handshake",
                ));
            }
            read_buf.extend_from_slice(&tmp[..n]);

            if let Some(result) = parse_101_response(&read_buf) {
                let response = result?;
                if response.status != 101 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("expected 101 Switching Protocols, got {}", response.status),
                    ));
                }
                verify_accept(response.accept.as_deref(), &key_base64)?;
                body_offset = response.body_offset;
                break;
            }
        }

        // Any bytes after the HTTP headers are the start of WebSocket frame data
        let leftover = &read_buf[body_offset..];
        if !leftover.is_empty() {
            println!("[ws] handshake leftover: {} bytes", leftover.len());
        }
        Ok(WebSocketConnection::new(stream, leftover))
    }
}
