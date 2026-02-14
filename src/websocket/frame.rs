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

//! WebSocket frame format (RFC 6455 §5): parser for receive, encoder for send (with masking).

use bytes::{Buf, BufMut, BytesMut};
use std::io;

// Opcodes
#[allow(dead_code)]
pub const OP_CONTINUATION: u8 = 0;
pub const OP_TEXT: u8 = 1;
pub const OP_BINARY: u8 = 2;
pub const OP_CLOSE: u8 = 8;
pub const OP_PING: u8 = 9;
pub const OP_PONG: u8 = 10;

/// Max payload length we accept for data frames (64 KiB). Control frames are ≤125.
pub const MAX_FRAME_PAYLOAD: usize = 65536;

/// Callback for completed frames (receive path).
pub trait FrameHandler {
    fn frame(&mut self, opcode: u8, fin: bool, data: &[u8]);
}

/// Push parser for WebSocket frames (server → client: no masking).
pub struct FrameParser {
    state: FrameState,
    opcode: u8,
    fin: bool,
    payload_len: u64,
    payload_read: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FrameState {
    Header1,
    ExtendedLen2,
    ExtendedLen8,
    Payload,
}

impl FrameParser {
    pub fn new() -> Self {
        Self {
            state: FrameState::Header1,
            opcode: 0,
            fin: false,
            payload_len: 0,
            payload_read: 0,
        }
    }

    /// Feed bytes from the stream. Returns Ok(()) when more data is needed or a frame was dispatched.
    pub fn receive<H: FrameHandler>(
        &mut self,
        buf: &mut BytesMut,
        handler: &mut H,
    ) -> Result<(), io::Error> {
        loop {
            match self.state {
                FrameState::Header1 => {
                    if buf.len() < 2 {
                        return Ok(());
                    }
                    let b0 = buf.get_u8();
                    let b1 = buf.get_u8();
                    self.fin = (b0 & 0x80) != 0;
                    self.opcode = b0 & 0x0f;
                    let mask = (b1 & 0x80) != 0;
                    let len7 = b1 & 0x7f;
                    if mask {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "server frame must not be masked",
                        ));
                    }
                    if len7 == 126 {
                        self.state = FrameState::ExtendedLen2;
                    } else if len7 == 127 {
                        self.state = FrameState::ExtendedLen8;
                    } else {
                        self.payload_len = len7 as u64;
                        self.payload_read = 0;
                        self.state = FrameState::Payload;
                    }
                }
                FrameState::ExtendedLen2 => {
                    if buf.len() < 2 {
                        return Ok(());
                    }
                    self.payload_len = buf.get_u16() as u64;
                    self.payload_read = 0;
                    self.state = FrameState::Payload;
                }
                FrameState::ExtendedLen8 => {
                    if buf.len() < 8 {
                        return Ok(());
                    }
                    self.payload_len = buf.get_u64();
                    self.payload_read = 0;
                    self.state = FrameState::Payload;
                }
                FrameState::Payload => {
                    let need = (self.payload_len - self.payload_read) as usize;
                    if need == 0 {
                        // Empty payload (e.g. ping with no data)
                        handler.frame(self.opcode, self.fin, &[]);
                        self.state = FrameState::Header1;
                        continue;
                    }
                    if buf.len() < need {
                        return Ok(());
                    }
                    let payload = buf.split_to(need);
                    let is_control = self.opcode == OP_CLOSE
                        || self.opcode == OP_PING
                        || self.opcode == OP_PONG;
                    if is_control && payload.len() > 125 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "control frame payload too long",
                        ));
                    }
                    if !is_control && payload.len() > MAX_FRAME_PAYLOAD {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "data frame payload too long",
                        ));
                    }
                    handler.frame(self.opcode, self.fin, &payload);
                    self.state = FrameState::Header1;
                    continue;
                }
            }
        }
    }
}

/// Encode and write one frame (client → server: must mask). Uses `mask_key` (4 bytes) for XOR.
pub fn encode_frame(
    opcode: u8,
    payload: &[u8],
    mask_key: &[u8; 4],
    out: &mut BytesMut,
) -> io::Result<()> {
    let len = payload.len();
    if len > MAX_FRAME_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "payload too long",
        ));
    }
    let fin: u8 = 0x80;
    out.put_u8(fin | (opcode & 0x0f));
    if len < 126 {
        out.put_u8(0x80 | (len as u8));
    } else if len < 65536 {
        out.put_u8(0x80 | 126);
        out.put_u16(len as u16);
    } else {
        out.put_u8(0x80 | 127);
        out.put_u64(len as u64);
    }
    out.put_slice(mask_key);
    for (i, &b) in payload.iter().enumerate() {
        out.put_u8(b ^ mask_key[i % 4]);
    }
    Ok(())
}
