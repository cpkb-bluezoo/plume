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

//! HTTP response handler trait (Gumdrop-shaped callbacks).
//!
//! Events: status -> headers -> start_body -> body_chunk (xn) -> end_body -> trailer (xn) -> complete / failed.

use crate::http::response::Response;

/// Handler for HTTP response events (push model). Connection drives this as data arrives.
///
/// Flow for a response with body:
/// 1. `ok(response)` or `error(response)` -- status received
/// 2. `header(name, value)` -- for each response header
/// 3. `start_body()` -- body begins
/// 4. `body_chunk(data)` -- for each chunk of body data
/// 5. `end_body()` -- body complete
/// 6. `header(name, value)` -- for each trailer (if any)
/// 7. `complete()` -- response fully complete
///
/// On connection/protocol failure only `failed(error)` is called.
pub trait ResponseHandler: Send + 'static {
    fn ok(&mut self, response: &Response);
    fn error(&mut self, response: &Response);
    fn header(&mut self, name: &str, value: &str);
    fn start_body(&mut self);
    fn body_chunk(&mut self, data: &[u8]);
    fn end_body(&mut self);
    fn complete(&mut self);
    fn failed(&mut self, error: &std::io::Error);

    /// Called by the connection task after writing each request body chunk
    /// to the socket. Override for upload progress reporting. Default: no-op.
    fn request_body_written(&mut self, _bytes_written: u64, _total_bytes: u64) {}
}
