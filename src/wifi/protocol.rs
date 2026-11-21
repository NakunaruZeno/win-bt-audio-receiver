pub const MAGIC: u16 = 0xA55A;
pub const HEADER_SIZE: usize = 12;
pub const DEFAULT_SAMPLE_RATE: u32 = 48_000;
pub const DEFAULT_CHANNELS: u8 = 2;
pub const FRAMES_PER_PACKET: usize = 240;
pub const SAMPLES_PER_PACKET: usize = FRAMES_PER_PACKET * (DEFAULT_CHANNELS as usize);
pub const PCM_BYTES_PER_PACKET: usize = SAMPLES_PER_PACKET * 2;
pub const MAX_UDP_PACKET_SIZE: usize = HEADER_SIZE + PCM_BYTES_PER_PACKET;

#[cfg(test)]
pub const FLAG_AUDIO: u8 = 0x00;
pub const FLAG_KEEPALIVE: u8 = 0x01;
pub const FLAG_HELLO: u8 = 0x02;
pub const FLAG_BYE: u8 = 0x04;

/// 4-byte ACK sent back to the Android Companion App on `FLAG_HELLO`
/// so the phone confirms LAN reachability immediately.
pub const ACK_PACKET: [u8; 4] = [0x5A, 0xA5, 0x00, 0x01];

pub const DEFAULT_UDP_PORT: u16 = 19480;
pub const DISCOVERY_UDP_PORT: u16 = 19481;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PacketHeader {
    pub seq: u32,
    pub sample_rate: u32,
    pub channels: u8,
    pub flags: u8,
}

impl PacketHeader {
    #[allow(dead_code)]
    pub fn encode(&self, out: &mut [u8; HEADER_SIZE]) {
        out[0..2].copy_from_slice(&MAGIC.to_le_bytes());
        out[2..6].copy_from_slice(&self.seq.to_le_bytes());
        out[6..10].copy_from_slice(&self.sample_rate.to_le_bytes());
        out[10] = self.channels;
        out[11] = self.flags;
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < HEADER_SIZE {
            return None;
        }
        let magic = u16::from_le_bytes([buf[0], buf[1]]);
        if magic != MAGIC {
            return None;
        }
        let seq = u32::from_le_bytes([buf[2], buf[3], buf[4], buf[5]]);
        let sample_rate = u32::from_le_bytes([buf[6], buf[7], buf[8], buf[9]]);
        let channels = buf[10];
        let flags = buf[11];

        Some(Self {
            seq,
            sample_rate,
            channels,
            flags,
        })
    }

    pub fn is_keepalive(&self) -> bool {
        (self.flags & FLAG_KEEPALIVE) != 0
    }

    pub fn is_hello(&self) -> bool {
        (self.flags & FLAG_HELLO) != 0
    }

    pub fn is_bye(&self) -> bool {
        (self.flags & FLAG_BYE) != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_header_encode_decode() {
        let orig = PacketHeader {
            seq: 42_123,
            sample_rate: DEFAULT_SAMPLE_RATE,
            channels: DEFAULT_CHANNELS,
            flags: FLAG_AUDIO,
        };
        let mut raw = [0u8; HEADER_SIZE];
        orig.encode(&mut raw);

        let parsed = PacketHeader::decode(&raw).expect("valid header");
        assert_eq!(parsed, orig);
        assert!(!parsed.is_keepalive());
        assert!(!parsed.is_hello());
        assert!(!parsed.is_bye());
    }

    #[test]
    fn decodes_hello_and_bye_control_flags() {
        let hello = PacketHeader {
            seq: 0,
            sample_rate: DEFAULT_SAMPLE_RATE,
            channels: DEFAULT_CHANNELS,
            flags: FLAG_HELLO,
        };
        let mut raw = [0u8; HEADER_SIZE];
        hello.encode(&mut raw);
        let parsed = PacketHeader::decode(&raw).unwrap();
        assert!(parsed.is_hello());
        assert!(!parsed.is_bye());

        let bye = PacketHeader {
            flags: FLAG_BYE,
            ..hello
        };
        bye.encode(&mut raw);
        let parsed_bye = PacketHeader::decode(&raw).unwrap();
        assert!(parsed_bye.is_bye());
    }

    #[test]
    fn rejects_invalid_magic_or_short_packet() {
        assert!(PacketHeader::decode(&[0u8; 6]).is_none());
        let mut bad = [0u8; HEADER_SIZE];
        bad[0] = 0x12;
        bad[1] = 0x34;
        assert!(PacketHeader::decode(&bad).is_none());
    }
}
