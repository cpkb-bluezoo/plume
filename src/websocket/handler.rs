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

//! WebSocket handler trait (callback-based).

/// Handler for WebSocket events (push model). Connection drives this as frames arrive.
#[allow(dead_code)]
pub trait WebSocketHandler {
    /// Handshake succeeded; connection is now in WebSocket frame mode.
    fn connected(&mut self);

    /// Text frame payload. Data is valid only for the duration of the call.
    fn text_frame(&mut self, data: &[u8]);

    /// Binary frame payload.
    fn binary_frame(&mut self, data: &[u8]);

    /// Close frame (optional code + reason). Connection will close after return.
    fn close(&mut self, code: Option<u16>, reason: &str);

    /// Ping received. Implementation may send Pong automatically or handler can send later.
    fn ping(&mut self, data: &[u8]);

    /// Pong received (e.g. in response to our Ping).
    fn pong(&mut self, data: &[u8]);

    /// Connection or protocol error.
    fn failed(&mut self, error: &std::io::Error);

    /// If true, the connection's run() loop will exit after the current frame. Default false.
    fn should_stop(&self) -> bool {
        false
    }
}
