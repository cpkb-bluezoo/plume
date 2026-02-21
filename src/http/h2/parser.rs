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

//! HTTP/2 frame push parser: consumes complete frames from a buffer,
//! dispatches to H2FrameHandler.

use bytes::{Buf, Bytes, BytesMut};
use std::io;

use super::frame::*;
use super::handler::H2FrameHandler;

/// Push parser for HTTP/2 frames. Feed bytes via `receive`; handler is
/// invoked for each complete frame.
pub struct H2Parser {
    max_frame_size: usize,
}

impl H2Parser {
    pub fn new() -> Self {
        Self {
            max_frame_size: DEFAULT_MAX_FRAME_SIZE,
        }
    }

    #[allow(dead_code)]
    pub fn set_max_frame_size(&mut self, size: usize) {
        assert!(
            (MIN_MAX_FRAME_SIZE..=MAX_MAX_FRAME_SIZE).contains(&size),
            "max frame size out of range"
        );
        self.max_frame_size = size;
    }

    /// Consume as many complete frames as possible from buf. Partial frame
    /// data is left in buf.
    pub fn receive<H: H2FrameHandler>(
        &mut self,
        buf: &mut BytesMut,
        handler: &mut H,
    ) -> Result<(), io::Error> {
        while buf.len() >= FRAME_HEADER_LENGTH {
            let length =
                (buf[0] as usize) << 16 | (buf[1] as usize) << 8 | (buf[2] as usize);
            if length > self.max_frame_size {
                handler.frame_error(
                    ERROR_FRAME_SIZE_ERROR,
                    0,
                    format!(
                        "Frame size {} exceeds max {}",
                        length, self.max_frame_size
                    ),
                );
                return Ok(());
            }
            if buf.len() < FRAME_HEADER_LENGTH + length {
                return Ok(());
            }
            let frame_type = buf[3];
            let flags = buf[4];
            let stream_id = ((buf[5] & 0x7f) as u32) << 24
                | (buf[6] as u32) << 16
                | (buf[7] as u32) << 8
                | (buf[8] as u32);

            buf.advance(FRAME_HEADER_LENGTH);
            let payload = buf.split_to(length);
            let payload_bytes = payload.freeze();

            dispatch_frame(frame_type, flags, stream_id, payload_bytes, handler)?;
        }
        Ok(())
    }
}

impl Default for H2Parser {
    fn default() -> Self {
        Self::new()
    }
}

fn dispatch_frame<H: H2FrameHandler>(
    frame_type: u8,
    flags: u8,
    stream_id: u32,
    payload: Bytes,
    handler: &mut H,
) -> Result<(), io::Error> {
    match frame_type {
        TYPE_DATA => parse_data_frame(flags, stream_id, payload, handler),
        TYPE_HEADERS => parse_headers_frame(flags, stream_id, payload, handler),
        TYPE_PRIORITY => parse_priority_frame(stream_id, payload, handler),
        TYPE_RST_STREAM => parse_rst_stream_frame(stream_id, payload, handler),
        TYPE_SETTINGS => parse_settings_frame(flags, stream_id, payload, handler),
        TYPE_PUSH_PROMISE => parse_push_promise_frame(flags, stream_id, payload, handler),
        TYPE_PING => parse_ping_frame(flags, stream_id, payload, handler),
        TYPE_GOAWAY => parse_goaway_frame(stream_id, payload, handler),
        TYPE_WINDOW_UPDATE => parse_window_update_frame(stream_id, payload, handler),
        TYPE_CONTINUATION => parse_continuation_frame(flags, stream_id, payload, handler),
        _ => Ok(()),
    }
}

fn parse_data_frame<H: H2FrameHandler>(
    flags: u8,
    stream_id: u32,
    mut payload: Bytes,
    handler: &mut H,
) -> Result<(), io::Error> {
    if stream_id == 0 {
        handler.frame_error(
            ERROR_PROTOCOL_ERROR,
            0,
            "DATA frame with stream ID 0".into(),
        );
        return Ok(());
    }
    let end_stream = (flags & FLAG_END_STREAM) != 0;
    let padded = (flags & FLAG_PADDED) != 0;
    let data = if padded {
        let pad_len = payload.get_u8() as usize;
        if payload.len() < pad_len {
            handler.frame_error(
                ERROR_PROTOCOL_ERROR,
                stream_id,
                "DATA frame padding exceeds payload".into(),
            );
            return Ok(());
        }
        payload.split_to(payload.len() - pad_len)
    } else {
        payload
    };
    handler.data_frame_received(stream_id, end_stream, data);
    Ok(())
}

fn parse_headers_frame<H: H2FrameHandler>(
    flags: u8,
    stream_id: u32,
    mut payload: Bytes,
    handler: &mut H,
) -> Result<(), io::Error> {
    if stream_id == 0 {
        handler.frame_error(
            ERROR_PROTOCOL_ERROR,
            0,
            "HEADERS frame with stream ID 0".into(),
        );
        return Ok(());
    }
    let end_stream = (flags & FLAG_END_STREAM) != 0;
    let end_headers = (flags & FLAG_END_HEADERS) != 0;
    let priority = (flags & FLAG_PRIORITY) != 0;
    let padded = (flags & FLAG_PADDED) != 0;

    let pad_len = if padded {
        if payload.is_empty() {
            handler.frame_error(
                ERROR_PROTOCOL_ERROR,
                stream_id,
                "HEADERS frame PADDED but no pad length".into(),
            );
            return Ok(());
        }
        let pl = payload.get_u8() as usize;
        if payload.len() < pl {
            handler.frame_error(
                ERROR_PROTOCOL_ERROR,
                stream_id,
                "HEADERS frame padding exceeds payload".into(),
            );
            return Ok(());
        }
        pl
    } else {
        0
    };

    let (stream_dependency, exclusive, weight) = if priority {
        if payload.len() < 5 {
            handler.frame_error(
                ERROR_FRAME_SIZE_ERROR,
                stream_id,
                "HEADERS frame with PRIORITY too short".into(),
            );
            return Ok(());
        }
        let b0 = payload.get_u8();
        let exclusive = (b0 & 0x80) != 0;
        let stream_dependency = (b0 as u32 & 0x7f) << 24
            | (payload.get_u8() as u32) << 16
            | (payload.get_u8() as u32) << 8
            | (payload.get_u8() as u32);
        let weight = payload.get_u8().saturating_add(1);
        (stream_dependency, exclusive, weight)
    } else {
        (0u32, false, 16u8)
    };

    let header_len = payload.len().saturating_sub(pad_len);
    let header_block = payload.split_to(header_len);
    handler.headers_frame_received(
        stream_id,
        end_stream,
        end_headers,
        stream_dependency,
        exclusive,
        weight,
        header_block,
    );
    Ok(())
}

fn parse_priority_frame<H: H2FrameHandler>(
    stream_id: u32,
    payload: Bytes,
    handler: &mut H,
) -> Result<(), io::Error> {
    if stream_id == 0 {
        handler.frame_error(
            ERROR_PROTOCOL_ERROR,
            0,
            "PRIORITY frame with stream ID 0".into(),
        );
        return Ok(());
    }
    if payload.len() != 5 {
        handler.frame_error(
            ERROR_FRAME_SIZE_ERROR,
            stream_id,
            "PRIORITY frame must be 5 bytes".into(),
        );
        return Ok(());
    }
    let mut p = payload;
    let b0 = p.get_u8();
    let exclusive = (b0 & 0x80) != 0;
    let stream_dependency = (b0 as u32 & 0x7f) << 24
        | (p.get_u8() as u32) << 16
        | (p.get_u8() as u32) << 8
        | (p.get_u8() as u32);
    let weight = p.get_u8().saturating_add(1);
    handler.priority_frame_received(stream_id, stream_dependency, exclusive, weight);
    Ok(())
}

fn parse_rst_stream_frame<H: H2FrameHandler>(
    stream_id: u32,
    payload: Bytes,
    handler: &mut H,
) -> Result<(), io::Error> {
    if stream_id == 0 {
        handler.frame_error(
            ERROR_PROTOCOL_ERROR,
            0,
            "RST_STREAM frame with stream ID 0".into(),
        );
        return Ok(());
    }
    if payload.len() != 4 {
        handler.frame_error(
            ERROR_FRAME_SIZE_ERROR,
            stream_id,
            "RST_STREAM frame must be 4 bytes".into(),
        );
        return Ok(());
    }
    let mut p = payload;
    let error_code = (p.get_u8() as u32) << 24
        | (p.get_u8() as u32) << 16
        | (p.get_u8() as u32) << 8
        | (p.get_u8() as u32);
    handler.rst_stream_frame_received(stream_id, error_code);
    Ok(())
}

fn parse_settings_frame<H: H2FrameHandler>(
    flags: u8,
    stream_id: u32,
    payload: Bytes,
    handler: &mut H,
) -> Result<(), io::Error> {
    if stream_id != 0 {
        handler.frame_error(
            ERROR_PROTOCOL_ERROR,
            stream_id,
            "SETTINGS frame with non-zero stream ID".into(),
        );
        return Ok(());
    }
    let ack = (flags & FLAG_ACK) != 0;
    if ack && !payload.is_empty() {
        handler.frame_error(
            ERROR_FRAME_SIZE_ERROR,
            0,
            "SETTINGS ACK frame must be empty".into(),
        );
        return Ok(());
    }
    if payload.len() % 6 != 0 {
        handler.frame_error(
            ERROR_FRAME_SIZE_ERROR,
            0,
            "SETTINGS frame size must be multiple of 6".into(),
        );
        return Ok(());
    }
    let mut settings = Vec::new();
    let mut p = payload;
    while p.len() >= 6 {
        let id = (p.get_u8() as u16) << 8 | (p.get_u8() as u16);
        let value = (p.get_u8() as u32) << 24
            | (p.get_u8() as u32) << 16
            | (p.get_u8() as u32) << 8
            | (p.get_u8() as u32);
        settings.push((id, value));
    }
    handler.settings_frame_received(ack, settings);
    Ok(())
}

fn parse_push_promise_frame<H: H2FrameHandler>(
    flags: u8,
    stream_id: u32,
    mut payload: Bytes,
    handler: &mut H,
) -> Result<(), io::Error> {
    if stream_id == 0 {
        handler.frame_error(
            ERROR_PROTOCOL_ERROR,
            0,
            "PUSH_PROMISE frame with stream ID 0".into(),
        );
        return Ok(());
    }
    let end_headers = (flags & FLAG_END_HEADERS) != 0;
    let padded = (flags & FLAG_PADDED) != 0;
    let mut end_pos = payload.len();
    if padded {
        let pad_len = payload.get_u8() as usize;
        if end_pos < 1 + pad_len {
            handler.frame_error(
                ERROR_PROTOCOL_ERROR,
                stream_id,
                "PUSH_PROMISE frame padding exceeds payload".into(),
            );
            return Ok(());
        }
        end_pos -= 1 + pad_len;
    }
    if payload.len() < 4 {
        handler.frame_error(
            ERROR_FRAME_SIZE_ERROR,
            stream_id,
            "PUSH_PROMISE frame too short".into(),
        );
        return Ok(());
    }
    let promised_stream_id = ((payload.get_u8() & 0x7f) as u32) << 24
        | (payload.get_u8() as u32) << 16
        | (payload.get_u8() as u32) << 8
        | (payload.get_u8() as u32);
    let header_len = end_pos
        .saturating_sub(4)
        .saturating_sub(if padded { 1 } else { 0 });
    let header_block = payload.split_to(header_len.min(payload.len()));
    handler.push_promise_frame_received(
        stream_id,
        promised_stream_id,
        end_headers,
        header_block,
    );
    Ok(())
}

fn parse_ping_frame<H: H2FrameHandler>(
    flags: u8,
    stream_id: u32,
    payload: Bytes,
    handler: &mut H,
) -> Result<(), io::Error> {
    if stream_id != 0 {
        handler.frame_error(
            ERROR_PROTOCOL_ERROR,
            stream_id,
            "PING frame with non-zero stream ID".into(),
        );
        return Ok(());
    }
    if payload.len() != 8 {
        handler.frame_error(
            ERROR_FRAME_SIZE_ERROR,
            0,
            "PING frame must be 8 bytes".into(),
        );
        return Ok(());
    }
    let ack = (flags & FLAG_ACK) != 0;
    let mut p = payload;
    let opaque = (p.get_u8() as u64) << 56
        | (p.get_u8() as u64) << 48
        | (p.get_u8() as u64) << 40
        | (p.get_u8() as u64) << 32
        | (p.get_u8() as u64) << 24
        | (p.get_u8() as u64) << 16
        | (p.get_u8() as u64) << 8
        | (p.get_u8() as u64);
    handler.ping_frame_received(ack, opaque);
    Ok(())
}

fn parse_goaway_frame<H: H2FrameHandler>(
    stream_id: u32,
    payload: Bytes,
    handler: &mut H,
) -> Result<(), io::Error> {
    if stream_id != 0 {
        handler.frame_error(
            ERROR_PROTOCOL_ERROR,
            stream_id,
            "GOAWAY frame with non-zero stream ID".into(),
        );
        return Ok(());
    }
    if payload.len() < 8 {
        handler.frame_error(
            ERROR_FRAME_SIZE_ERROR,
            0,
            "GOAWAY frame must be at least 8 bytes".into(),
        );
        return Ok(());
    }
    let mut p = payload;
    let last_stream_id = ((p.get_u8() & 0x7f) as u32) << 24
        | (p.get_u8() as u32) << 16
        | (p.get_u8() as u32) << 8
        | (p.get_u8() as u32);
    let error_code = (p.get_u8() as u32) << 24
        | (p.get_u8() as u32) << 16
        | (p.get_u8() as u32) << 8
        | (p.get_u8() as u32);
    let debug_data = p;
    handler.goaway_frame_received(last_stream_id, error_code, debug_data);
    Ok(())
}

fn parse_window_update_frame<H: H2FrameHandler>(
    stream_id: u32,
    payload: Bytes,
    handler: &mut H,
) -> Result<(), io::Error> {
    if payload.len() != 4 {
        handler.frame_error(
            ERROR_FRAME_SIZE_ERROR,
            stream_id,
            "WINDOW_UPDATE frame must be 4 bytes".into(),
        );
        return Ok(());
    }
    let mut p = payload;
    let increment = ((p.get_u8() & 0x7f) as u32) << 24
        | (p.get_u8() as u32) << 16
        | (p.get_u8() as u32) << 8
        | (p.get_u8() as u32);
    if increment == 0 {
        handler.frame_error(
            ERROR_PROTOCOL_ERROR,
            stream_id,
            "WINDOW_UPDATE increment must be non-zero".into(),
        );
        return Ok(());
    }
    handler.window_update_frame_received(stream_id, increment);
    Ok(())
}

fn parse_continuation_frame<H: H2FrameHandler>(
    flags: u8,
    stream_id: u32,
    payload: Bytes,
    handler: &mut H,
) -> Result<(), io::Error> {
    if stream_id == 0 {
        handler.frame_error(
            ERROR_PROTOCOL_ERROR,
            0,
            "CONTINUATION frame with stream ID 0".into(),
        );
        return Ok(());
    }
    let end_headers = (flags & FLAG_END_HEADERS) != 0;
    handler.continuation_frame_received(stream_id, end_headers, payload);
    Ok(())
}
