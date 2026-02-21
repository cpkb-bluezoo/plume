/*
 * frame.rs
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

//! HTTP/2 frame type and flag constants (RFC 7540).

pub const TYPE_DATA: u8 = 0x0;
pub const TYPE_HEADERS: u8 = 0x1;
pub const TYPE_PRIORITY: u8 = 0x2;
pub const TYPE_RST_STREAM: u8 = 0x3;
pub const TYPE_SETTINGS: u8 = 0x4;
pub const TYPE_PUSH_PROMISE: u8 = 0x5;
pub const TYPE_PING: u8 = 0x6;
pub const TYPE_GOAWAY: u8 = 0x7;
pub const TYPE_WINDOW_UPDATE: u8 = 0x8;
pub const TYPE_CONTINUATION: u8 = 0x9;

pub const FLAG_ACK: u8 = 0x1;
pub const FLAG_END_STREAM: u8 = 0x1;
pub const FLAG_END_HEADERS: u8 = 0x4;
pub const FLAG_PADDED: u8 = 0x8;
pub const FLAG_PRIORITY: u8 = 0x20;

pub const ERROR_NO_ERROR: u32 = 0x0;
pub const ERROR_PROTOCOL_ERROR: u32 = 0x1;
#[allow(dead_code)]
pub const ERROR_INTERNAL_ERROR: u32 = 0x2;
#[allow(dead_code)]
pub const ERROR_FLOW_CONTROL_ERROR: u32 = 0x3;
#[allow(dead_code)]
pub const ERROR_SETTINGS_TIMEOUT: u32 = 0x4;
#[allow(dead_code)]
pub const ERROR_STREAM_CLOSED: u32 = 0x5;
pub const ERROR_FRAME_SIZE_ERROR: u32 = 0x6;
#[allow(dead_code)]
pub const ERROR_REFUSED_STREAM: u32 = 0x7;
#[allow(dead_code)]
pub const ERROR_CANCEL: u32 = 0x8;
#[allow(dead_code)]
pub const ERROR_COMPRESSION_ERROR: u32 = 0x9;
#[allow(dead_code)]
pub const ERROR_CONNECT_ERROR: u32 = 0xa;
#[allow(dead_code)]
pub const ERROR_ENHANCE_YOUR_CALM: u32 = 0xb;
#[allow(dead_code)]
pub const ERROR_INADEQUATE_SECURITY: u32 = 0xc;
#[allow(dead_code)]
pub const ERROR_HTTP_1_1_REQUIRED: u32 = 0xd;

pub const SETTINGS_HEADER_TABLE_SIZE: u16 = 0x1;
#[allow(dead_code)]
pub const SETTINGS_ENABLE_PUSH: u16 = 0x2;
#[allow(dead_code)]
pub const SETTINGS_MAX_CONCURRENT_STREAMS: u16 = 0x3;
pub const SETTINGS_INITIAL_WINDOW_SIZE: u16 = 0x4;
pub const SETTINGS_MAX_FRAME_SIZE: u16 = 0x5;
#[allow(dead_code)]
pub const SETTINGS_MAX_HEADER_LIST_SIZE: u16 = 0x6;

pub const FRAME_HEADER_LENGTH: usize = 9;
pub const DEFAULT_MAX_FRAME_SIZE: usize = 16384;
pub const MIN_MAX_FRAME_SIZE: usize = 16384;
pub const MAX_MAX_FRAME_SIZE: usize = 16_777_215;

pub fn error_to_string(code: u32) -> &'static str {
    match code {
        ERROR_NO_ERROR => "NO_ERROR",
        ERROR_PROTOCOL_ERROR => "PROTOCOL_ERROR",
        ERROR_INTERNAL_ERROR => "INTERNAL_ERROR",
        ERROR_FLOW_CONTROL_ERROR => "FLOW_CONTROL_ERROR",
        ERROR_SETTINGS_TIMEOUT => "SETTINGS_TIMEOUT",
        ERROR_STREAM_CLOSED => "STREAM_CLOSED",
        ERROR_FRAME_SIZE_ERROR => "FRAME_SIZE_ERROR",
        ERROR_REFUSED_STREAM => "REFUSED_STREAM",
        ERROR_CANCEL => "CANCEL",
        ERROR_COMPRESSION_ERROR => "COMPRESSION_ERROR",
        ERROR_CONNECT_ERROR => "CONNECT_ERROR",
        ERROR_ENHANCE_YOUR_CALM => "ENHANCE_YOUR_CALM",
        ERROR_INADEQUATE_SECURITY => "INADEQUATE_SECURITY",
        ERROR_HTTP_1_1_REQUIRED => "HTTP_1_1_REQUIRED",
        _ => "UNKNOWN",
    }
}
