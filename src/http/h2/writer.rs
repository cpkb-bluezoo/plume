/*
 * writer.rs
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

//! HTTP/2 frame writer: serializes frames into a buffer.

use bytes::{BufMut, Bytes, BytesMut};
use std::io;

use super::frame::*;

/// Writes HTTP/2 frames into a BytesMut. Caller is responsible for sending
/// the buffer to the stream.
pub struct H2Writer {
    buf: BytesMut,
}

impl H2Writer {
    pub fn new() -> Self {
        Self {
            buf: BytesMut::with_capacity(16384 + FRAME_HEADER_LENGTH),
        }
    }

    fn write_frame_header(
        &mut self,
        length: usize,
        frame_type: u8,
        flags: u8,
        stream_id: u32,
    ) {
        self.buf.put_u8((length >> 16) as u8);
        self.buf.put_u8((length >> 8) as u8);
        self.buf.put_u8(length as u8);
        self.buf.put_u8(frame_type);
        self.buf.put_u8(flags);
        self.buf.put_u32(stream_id);
    }

    pub fn write_data(
        &mut self,
        stream_id: u32,
        data: &[u8],
        end_stream: bool,
    ) -> io::Result<usize> {
        if stream_id == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "DATA frame stream_id must be non-zero",
            ));
        }
        let flags = if end_stream { FLAG_END_STREAM } else { 0 };
        let len = data.len();
        self.write_frame_header(len, TYPE_DATA, flags, stream_id);
        self.buf.extend_from_slice(data);
        Ok(len)
    }

    pub fn write_headers(
        &mut self,
        stream_id: u32,
        header_block: &[u8],
        end_stream: bool,
        end_headers: bool,
    ) -> io::Result<()> {
        if stream_id == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "HEADERS frame stream_id must be non-zero",
            ));
        }
        let mut flags = 0u8;
        if end_stream {
            flags |= FLAG_END_STREAM;
        }
        if end_headers {
            flags |= FLAG_END_HEADERS;
        }
        self.write_frame_header(header_block.len(), TYPE_HEADERS, flags, stream_id);
        self.buf.extend_from_slice(header_block);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn write_headers_with_priority(
        &mut self,
        stream_id: u32,
        header_block: &[u8],
        end_stream: bool,
        end_headers: bool,
        stream_dependency: u32,
        weight: u8,
        exclusive: bool,
    ) -> io::Result<()> {
        if stream_id == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "HEADERS frame stream_id must be non-zero",
            ));
        }
        let mut flags = FLAG_PRIORITY;
        if end_stream {
            flags |= FLAG_END_STREAM;
        }
        if end_headers {
            flags |= FLAG_END_HEADERS;
        }
        let payload_len = 5 + header_block.len();
        self.write_frame_header(payload_len, TYPE_HEADERS, flags, stream_id);
        let dep = if exclusive {
            stream_dependency | 0x8000_0000
        } else {
            stream_dependency
        };
        self.buf.put_u32(dep);
        self.buf.put_u8(weight.saturating_sub(1));
        self.buf.extend_from_slice(header_block);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn write_rst_stream(&mut self, stream_id: u32, error_code: u32) -> io::Result<()> {
        if stream_id == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "RST_STREAM stream_id must be non-zero",
            ));
        }
        self.write_frame_header(4, TYPE_RST_STREAM, 0, stream_id);
        self.buf.put_u32(error_code);
        Ok(())
    }

    pub fn write_settings(&mut self, settings: &[(u16, u32)]) -> io::Result<()> {
        let payload_len = settings.len() * 6;
        self.write_frame_header(payload_len, TYPE_SETTINGS, 0, 0);
        for (id, value) in settings {
            self.buf.put_u16(*id);
            self.buf.put_u32(*value);
        }
        Ok(())
    }

    pub fn write_settings_ack(&mut self) -> io::Result<()> {
        self.write_frame_header(0, TYPE_SETTINGS, FLAG_ACK, 0);
        Ok(())
    }

    pub fn write_ping(&mut self, opaque_data: u64, ack: bool) -> io::Result<()> {
        let flags = if ack { FLAG_ACK } else { 0 };
        self.write_frame_header(8, TYPE_PING, flags, 0);
        self.buf.put_u64(opaque_data);
        Ok(())
    }

    #[allow(dead_code)]
    pub fn write_goaway(
        &mut self,
        last_stream_id: u32,
        error_code: u32,
        debug_data: &[u8],
    ) -> io::Result<()> {
        self.write_frame_header(8 + debug_data.len(), TYPE_GOAWAY, 0, 0);
        self.buf.put_u32(last_stream_id & 0x7fff_ffff);
        self.buf.put_u32(error_code);
        self.buf.extend_from_slice(debug_data);
        Ok(())
    }

    pub fn write_window_update(
        &mut self,
        stream_id: u32,
        increment: u32,
    ) -> io::Result<()> {
        if increment == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "WINDOW_UPDATE increment must be non-zero",
            ));
        }
        self.write_frame_header(4, TYPE_WINDOW_UPDATE, 0, stream_id);
        self.buf.put_u32(increment & 0x7fff_ffff);
        Ok(())
    }

    pub fn take_buffer(&mut self) -> Bytes {
        self.buf.split().freeze()
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }
}

impl Default for H2Writer {
    fn default() -> Self {
        Self::new()
    }
}
