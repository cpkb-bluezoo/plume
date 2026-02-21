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

//! HTTP/2 frame handler trait (callbacks for parsed frames).

use bytes::Bytes;

/// Callback for parsed HTTP/2 frames. Payloads are Bytes (zero-copy where
/// possible).
pub trait H2FrameHandler: Send {
    fn data_frame_received(&mut self, stream_id: u32, end_stream: bool, data: Bytes);
    fn headers_frame_received(
        &mut self,
        stream_id: u32,
        end_stream: bool,
        end_headers: bool,
        stream_dependency: u32,
        exclusive: bool,
        weight: u8,
        header_block_fragment: Bytes,
    );
    fn priority_frame_received(
        &mut self,
        stream_id: u32,
        stream_dependency: u32,
        exclusive: bool,
        weight: u8,
    );
    fn rst_stream_frame_received(&mut self, stream_id: u32, error_code: u32);
    fn settings_frame_received(&mut self, ack: bool, settings: Vec<(u16, u32)>);
    fn push_promise_frame_received(
        &mut self,
        stream_id: u32,
        promised_stream_id: u32,
        end_headers: bool,
        header_block_fragment: Bytes,
    );
    fn ping_frame_received(&mut self, ack: bool, opaque_data: u64);
    fn goaway_frame_received(&mut self, last_stream_id: u32, error_code: u32, debug_data: Bytes);
    fn window_update_frame_received(&mut self, stream_id: u32, window_size_increment: u32);
    fn continuation_frame_received(
        &mut self,
        stream_id: u32,
        end_headers: bool,
        header_block_fragment: Bytes,
    );
    fn frame_error(&mut self, error_code: u32, stream_id: u32, message: String);
}
