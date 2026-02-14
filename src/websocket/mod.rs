/*
 * mod.rs
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

//! WebSocket client (RFC 6455): handshake over HTTP/1.1, then frame-based send/recv.
//! Callback-based API: implement WebSocketHandler to receive frames.

mod client;
pub mod connection;
mod frame;
mod handler;
mod handshake;
pub mod stream;

pub use client::WebSocketClient;
pub use handler::WebSocketHandler;
