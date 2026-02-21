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

//! HTTP client: event-driven, push-parsed HTTP/1.1 and HTTP/2 client.
//!
//! Architecture (Gumdrop pattern):
//! - `HttpClient::new(host, port, secure)` creates a per-host client and
//!   spawns a background connection task.
//! - Factory methods (`get`, `put`, `post`, `delete`) return `HttpRequest`
//!   handles. Configure headers, then fire via `send()` or `start_body()`.
//! - Response data is pushed to a `ResponseHandler` as it arrives:
//!   ok/error -> headers -> body_chunk (xn) -> end_body -> complete.
//! - Body chunks flow into `JsonParser` -> `JsonContentHandler` for
//!   streaming response parsing (SAX-like pipeline).
//! - HTTP/2 via ALPN with HPACK header compression. HTTP/1.1 fallback
//!   with chunked encoding and persistent connections.

mod handler;
mod response;

pub mod h1;
pub mod h2;
pub mod hpack;

pub mod client;
pub mod request;

pub use client::HttpClient;
pub use handler::ResponseHandler;
pub use response::Response;
