/*
 * request.rs
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

//! HTTP request handle: accumulates method, path, headers. Fire via
//! `send()` (no body) or `start_body()` (streaming body).

use tokio::sync::mpsc;

use crate::http::client::Command;
use crate::http::ResponseHandler;

/// HTTP request handle. Returned by `HttpClient::get()`, etc.
/// Configure headers, then fire with `send()` or `start_body()`.
pub struct HttpRequest {
    cmd_tx: mpsc::UnboundedSender<Command>,
    method: String,
    path: String,
    headers: Vec<(String, String)>,
}

impl HttpRequest {
    pub(crate) fn new(
        cmd_tx: mpsc::UnboundedSender<Command>,
        method: &str,
        path: &str,
    ) -> Self {
        Self {
            cmd_tx,
            method: method.to_string(),
            path: path.to_string(),
            headers: Vec::new(),
        }
    }

    pub fn header(&mut self, name: &str, value: &str) -> &mut Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    /// Fire-and-forget: send request without body. Handler receives
    /// response events from the connection task.
    pub fn send(self, handler: Box<dyn ResponseHandler>) {
        let _ = self.cmd_tx.send(Command::SendRequest {
            method: self.method,
            path: self.path,
            headers: self.headers,
            handler,
            has_body: false,
        });
    }

    /// Fire-and-forget: send request headers, returns `RequestBody` for
    /// streaming body data. Handler receives response events.
    pub fn start_body(self, handler: Box<dyn ResponseHandler>) -> RequestBody {
        let tx = self.cmd_tx.clone();
        let _ = self.cmd_tx.send(Command::SendRequest {
            method: self.method,
            path: self.path,
            headers: self.headers,
            handler,
            has_body: true,
        });
        RequestBody { cmd_tx: tx }
    }
}

/// Handle for pushing body chunks after `start_body()`.
pub struct RequestBody {
    cmd_tx: mpsc::UnboundedSender<Command>,
}

impl RequestBody {
    /// Queue a body chunk for writing. Synchronous (does not block on
    /// network write). The connection task writes chunks and calls
    /// `handler.request_body_written()` after each write.
    pub fn body_content(&self, data: &[u8]) {
        let _ = self.cmd_tx.send(Command::BodyChunk {
            data: data.to_vec(),
        });
    }

    /// Signal end of request body.
    pub fn end_body(self) {
        let _ = self.cmd_tx.send(Command::EndBody);
    }
}
