//! Splits the client's TCP byte stream into whole MTProto transport packets
//! so each one goes out as its own WebSocket frame (Telegram's WS endpoints
//! expect packet-aligned frames).

use crate::obfs::ProtoTag;

pub struct PacketSplitter {
    proto: ProtoTag,
    cipher_buf: Vec<u8>,
    plain_buf: Vec<u8>,
    disabled: bool,
}

/// Outcome of looking at the next packet header in the buffer.
enum Next {
    /// Not enough bytes buffered yet.
    Incomplete,
    /// A whole packet of this many bytes (header included) is buffered.
    Packet(usize),
    /// Zero-length packet: framing is unknown, stop splitting (as upstream).
    Unsplittable,
}

impl PacketSplitter {
    pub fn new(proto: ProtoTag) -> Self {
        PacketSplitter {
            proto,
            cipher_buf: Vec::new(),
            plain_buf: Vec::new(),
            disabled: false,
        }
    }

    /// Feeds one chunk (`cipher` and its decrypted `plain`, equal length) and
    /// returns the cipher bytes of every packet completed by it.
    pub fn split(&mut self, cipher: &[u8], plain: &[u8]) -> Vec<Vec<u8>> {
        debug_assert_eq!(cipher.len(), plain.len());
        if cipher.is_empty() {
            return Vec::new();
        }
        if self.disabled {
            return vec![cipher.to_vec()];
        }
        self.cipher_buf.extend_from_slice(cipher);
        self.plain_buf.extend_from_slice(plain);

        let mut parts = Vec::new();
        let mut offset = 0;
        while offset < self.cipher_buf.len() {
            match self.next_packet(offset) {
                Next::Incomplete => break,
                Next::Packet(len) => {
                    parts.push(self.cipher_buf[offset..offset + len].to_vec());
                    offset += len;
                }
                Next::Unsplittable => {
                    parts.push(self.cipher_buf[offset..].to_vec());
                    offset = self.cipher_buf.len();
                    self.disabled = true;
                }
            }
        }
        self.cipher_buf.drain(..offset);
        self.plain_buf.drain(..offset);
        parts
    }

    /// Returns whatever is still buffered (an incomplete packet) at EOF.
    pub fn flush(&mut self) -> Option<Vec<u8>> {
        if self.cipher_buf.is_empty() {
            return None;
        }
        self.plain_buf.clear();
        Some(std::mem::take(&mut self.cipher_buf))
    }

    fn next_packet(&self, offset: usize) -> Next {
        let p = &self.plain_buf[offset..];
        let (header_len, payload_len) = match self.proto {
            ProtoTag::Abridged => {
                if p[0] == 0x7F || p[0] == 0xFF {
                    if p.len() < 4 {
                        return Next::Incomplete;
                    }
                    (4, u32::from_le_bytes([p[1], p[2], p[3], 0]) as usize * 4)
                } else {
                    (1, (p[0] & 0x7F) as usize * 4)
                }
            }
            ProtoTag::Intermediate | ProtoTag::PaddedIntermediate => {
                if p.len() < 4 {
                    return Next::Incomplete;
                }
                (
                    4,
                    (u32::from_le_bytes([p[0], p[1], p[2], p[3]]) & 0x7FFF_FFFF) as usize,
                )
            }
        };
        if payload_len == 0 {
            Next::Unsplittable
        } else if p.len() < header_len + payload_len {
            Next::Incomplete
        } else {
            Next::Packet(header_len + payload_len)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(header: &[u8], payload_len: usize) -> Vec<u8> {
        let mut p = header.to_vec();
        p.extend((0..payload_len).map(|i| i as u8 + 1));
        p
    }

    fn split(s: &mut PacketSplitter, data: &[u8]) -> Vec<Vec<u8>> {
        s.split(data, data)
    }

    #[test]
    fn abridged_short_header() {
        let p = packet(&[0x02], 8);
        assert_eq!(
            split(&mut PacketSplitter::new(ProtoTag::Abridged), &p),
            vec![p]
        );
    }

    #[test]
    fn abridged_long_header() {
        for first in [0x7F, 0xFF] {
            let p = packet(&[first, 0x01, 0x00, 0x00], 4);
            assert_eq!(
                split(&mut PacketSplitter::new(ProtoTag::Abridged), &p),
                vec![p]
            );
        }
    }

    #[test]
    fn intermediate_len_and_quickack_bit() {
        for proto in [ProtoTag::Intermediate, ProtoTag::PaddedIntermediate] {
            for last in [0x00, 0x80] {
                let p = packet(&[8, 0, 0, last], 8);
                assert_eq!(p.len(), 12);
                assert_eq!(split(&mut PacketSplitter::new(proto), &p), vec![p]);
            }
        }
    }

    #[test]
    fn two_packets_in_one_chunk() {
        let a = packet(&[4, 0, 0, 0], 4);
        let b = packet(&[8, 0, 0, 0], 8);
        let both = [a.clone(), b.clone()].concat();
        assert_eq!(
            split(&mut PacketSplitter::new(ProtoTag::Intermediate), &both),
            vec![a, b]
        );
    }

    #[test]
    fn header_split_across_calls() {
        let mut s = PacketSplitter::new(ProtoTag::Abridged);
        let p = packet(&[0x7F, 0x01, 0x00, 0x00], 4);
        assert!(split(&mut s, &p[..2]).is_empty());
        assert_eq!(split(&mut s, &p[2..]), vec![p]);
    }

    #[test]
    fn payload_split_across_calls() {
        let mut s = PacketSplitter::new(ProtoTag::Intermediate);
        let p = packet(&[8, 0, 0, 0], 8);
        assert!(split(&mut s, &p[..7]).is_empty());
        assert_eq!(split(&mut s, &p[7..]), vec![p]);
    }

    #[test]
    fn cipher_bytes_are_emitted_but_plain_decides() {
        let mut s = PacketSplitter::new(ProtoTag::Intermediate);
        let plain = packet(&[4, 0, 0, 0], 4);
        let cipher: Vec<u8> = plain.iter().map(|b| b ^ 0xAA).collect();
        assert_eq!(s.split(&cipher, &plain), vec![cipher]);
    }

    #[test]
    fn zero_length_disables_splitting() {
        let mut s = PacketSplitter::new(ProtoTag::Intermediate);
        let first = [vec![0, 0, 0, 0], vec![9, 9, 9]].concat();
        assert_eq!(split(&mut s, &first), vec![first.clone()]);
        let later = vec![4, 0, 0, 0, 1];
        assert_eq!(split(&mut s, &later), vec![later.clone()]);
        assert_eq!(s.flush(), None);
    }

    #[test]
    fn flush_returns_tail() {
        let mut s = PacketSplitter::new(ProtoTag::Intermediate);
        let p = packet(&[8, 0, 0, 0], 8);
        assert!(split(&mut s, &p[..5]).is_empty());
        assert_eq!(s.flush(), Some(p[..5].to_vec()));
        assert_eq!(s.flush(), None);
    }
}
