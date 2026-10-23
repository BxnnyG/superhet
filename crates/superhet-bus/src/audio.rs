//! Binary audio frame published on `sh.v1.dev.<dev>.audio.<vfo>`.
//!
//! Wire format version 1, all integers and floats little-endian, 42-byte header followed by one
//! Opus packet (one packet = one frame of audio, 20 ms at 48 kHz = 960 samples per channel):
//!
//! | offset | type | field |
//! |---|---|---|
//! | 0 | `u8[4]` | `magic` (= `"SHAU"`) |
//! | 4 | `u8` | `version` (= 1) |
//! | 5 | `u8` | `codec` (= 1: Opus) |
//! | 6 | `u8` | `channels` (1 = mono, 2 = interleaved stereo) |
//! | 7 | `u8` | `flags`: bit 0 = squelch open, bit 1 = stereo pilot locked |
//! | 8 | `u32` | `sample_rate` in Hz (= 48000) |
//! | 12 | `u32` | `vfo_id` |
//! | 16 | `u32` | `seq`, +1 per frame, wraps |
//! | 20 | `u64` | `sample_pos`, 48 kHz sample counter of the first sample, continuous per VFO |
//! | 28 | `u64` | `capture_unix_ns`, wall time at which the audio of the first sample was captured |
//! | 36 | `f32` | `power_dbfs`, channel power (S-meter) |
//! | 40 | `u16` | `payload_len` |
//! | 42 | `u8[payload_len]` | Opus packet |
//!
//! Audio is lossy live data: it is published on core NATS (no JetStream) and may be dropped.
//! Consumers detect loss through `seq` / `sample_pos`.

/// Current wire version.
pub const VERSION: u8 = 1;
/// Magic bytes at the start of every frame.
pub const MAGIC: [u8; 4] = *b"SHAU";
/// Size of the fixed header in bytes.
pub const HEADER_LEN: usize = 42;
/// Codec id: Opus.
pub const CODEC_OPUS: u8 = 1;
/// Flag bit 0: the squelch is open (or disabled), i.e. the frame carries real audio.
pub const FLAG_SQUELCH_OPEN: u8 = 0x01;
/// Flag bit 1: the VFO is a stereo broadcast with a locked pilot, i.e. the two channels of the
/// frame differ (only ever set on `channels = 2` frames).
pub const FLAG_STEREO: u8 = 0x02;
/// Samples per channel per frame at 48 kHz (20 ms).
pub const FRAME_SAMPLES: usize = 960;

/// Decoding failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// Shorter than the header, or the body length does not match `payload_len`.
    #[error("audio frame has wrong length")]
    Length,
    /// The magic bytes are wrong.
    #[error("not an audio frame")]
    Magic,
    /// Unknown wire version.
    #[error("unsupported audio frame version {0}")]
    Version(u8),
    /// Unknown codec id.
    #[error("unsupported audio codec {0}")]
    Codec(u8),
    /// Channel count other than 1 or 2.
    #[error("unsupported channel count {0}")]
    Channels(u8),
}

/// One audio frame.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioFrame {
    /// Channel count (1 mono, 2 interleaved stereo).
    pub channels: u8,
    /// Flag bits, see [`FLAG_SQUELCH_OPEN`] and [`FLAG_STEREO`].
    pub flags: u8,
    /// Sample rate in Hz.
    pub sample_rate: u32,
    /// VFO that produced the frame.
    pub vfo_id: u32,
    /// Frame counter.
    pub seq: u32,
    /// Running sample counter of the first sample.
    pub sample_pos: u64,
    /// Capture time of the first sample, Unix time in nanoseconds.
    pub capture_unix_ns: u64,
    /// Channel power in dBFS.
    pub power_dbfs: f32,
    /// Opus packet.
    pub payload: Vec<u8>,
}

impl AudioFrame {
    /// True if the squelch was open (or disabled).
    #[must_use]
    pub fn squelch_open(&self) -> bool {
        self.flags & FLAG_SQUELCH_OPEN != 0
    }

    /// True if the pilot of a stereo broadcast is locked (the channels carry different audio).
    #[must_use]
    pub fn stereo(&self) -> bool {
        self.flags & FLAG_STEREO != 0
    }

    /// Serialise to the wire format. A payload beyond `u16::MAX` bytes is not encoded (an Opus
    /// packet of a 20 ms frame is at most 1275 bytes).
    #[must_use]
    pub fn encode(&self) -> Vec<u8> {
        let n = self.payload.len().min(usize::from(u16::MAX));
        let mut out = Vec::with_capacity(HEADER_LEN + n);
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        out.push(CODEC_OPUS);
        out.push(self.channels);
        out.push(self.flags);
        out.extend_from_slice(&self.sample_rate.to_le_bytes());
        out.extend_from_slice(&self.vfo_id.to_le_bytes());
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&self.sample_pos.to_le_bytes());
        out.extend_from_slice(&self.capture_unix_ns.to_le_bytes());
        out.extend_from_slice(&self.power_dbfs.to_le_bytes());
        out.extend_from_slice(&(n as u16).to_le_bytes());
        out.extend_from_slice(&self.payload[..n]);
        out
    }

    /// Parse the wire format.
    ///
    /// # Errors
    /// Fails on truncated data, trailing bytes, wrong magic or unknown version/codec.
    pub fn decode(data: &[u8]) -> Result<Self, FrameError> {
        let header: &[u8; HEADER_LEN] = data
            .get(..HEADER_LEN)
            .and_then(|h| h.try_into().ok())
            .ok_or(FrameError::Length)?;
        if header[..4] != MAGIC {
            return Err(FrameError::Magic);
        }
        if header[4] != VERSION {
            return Err(FrameError::Version(header[4]));
        }
        if header[5] != CODEC_OPUS {
            return Err(FrameError::Codec(header[5]));
        }
        if !(1..=2).contains(&header[6]) {
            return Err(FrameError::Channels(header[6]));
        }
        let payload_len = usize::from(u16::from_le_bytes([header[40], header[41]]));
        if data.len() != HEADER_LEN + payload_len {
            return Err(FrameError::Length);
        }
        let u32_at =
            |o: usize| u32::from_le_bytes([header[o], header[o + 1], header[o + 2], header[o + 3]]);
        let u64_at = |o: usize| {
            let mut b = [0u8; 8];
            b.copy_from_slice(&header[o..o + 8]);
            u64::from_le_bytes(b)
        };
        Ok(Self {
            channels: header[6],
            flags: header[7],
            sample_rate: u32_at(8),
            vfo_id: u32_at(12),
            seq: u32_at(16),
            sample_pos: u64_at(20),
            capture_unix_ns: u64_at(28),
            power_dbfs: f32::from_bits(u32_at(36)),
            payload: data[HEADER_LEN..].to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AudioFrame {
        AudioFrame {
            channels: 1,
            flags: FLAG_SQUELCH_OPEN,
            sample_rate: 48_000,
            vfo_id: 7,
            seq: 0xDEAD_BEEF,
            sample_pos: 0x0123_4567_89AB_CDEF,
            capture_unix_ns: 1_700_000_000_123_456_789,
            power_dbfs: -42.5,
            payload: vec![1, 2, 3, 4, 5],
        }
    }

    #[test]
    fn round_trips() {
        let f = sample();
        let bytes = f.encode();
        assert_eq!(bytes.len(), HEADER_LEN + 5);
        assert_eq!(&bytes[..4], b"SHAU");
        assert_eq!(bytes[4], VERSION);
        assert_eq!(AudioFrame::decode(&bytes), Ok(f.clone()));
        assert!(f.squelch_open());
        let mut closed = f.clone();
        closed.flags = 0;
        closed.payload.clear();
        assert!(!closed.squelch_open());
        assert_eq!(AudioFrame::decode(&closed.encode()), Ok(closed.clone()));

        let mut stereo = closed;
        stereo.channels = 2;
        stereo.flags = FLAG_SQUELCH_OPEN | FLAG_STEREO;
        let decoded = AudioFrame::decode(&stereo.encode()).expect("decode");
        assert_eq!(decoded.channels, 2);
        assert!(decoded.stereo() && decoded.squelch_open());
        assert!(!f.stereo());
    }

    #[test]
    fn header_layout_is_little_endian() {
        let bytes = sample().encode();
        assert_eq!(&bytes[8..12], &48_000u32.to_le_bytes());
        assert_eq!(&bytes[12..16], &7u32.to_le_bytes());
        assert_eq!(&bytes[16..20], &0xDEAD_BEEFu32.to_le_bytes());
        assert_eq!(&bytes[20..28], &0x0123_4567_89AB_CDEFu64.to_le_bytes());
        assert_eq!(&bytes[36..40], &(-42.5f32).to_le_bytes());
        assert_eq!(&bytes[40..42], &5u16.to_le_bytes());
    }

    #[test]
    fn rejects_malformed() {
        let ok = sample().encode();
        assert_eq!(AudioFrame::decode(&ok[..10]), Err(FrameError::Length));
        assert_eq!(
            AudioFrame::decode(&ok[..ok.len() - 1]),
            Err(FrameError::Length)
        );
        let mut extra = ok.clone();
        extra.push(0);
        assert_eq!(AudioFrame::decode(&extra), Err(FrameError::Length));
        let mut v = ok.clone();
        v[0] = b'X';
        assert_eq!(AudioFrame::decode(&v), Err(FrameError::Magic));
        let mut v = ok.clone();
        v[4] = 2;
        assert_eq!(AudioFrame::decode(&v), Err(FrameError::Version(2)));
        let mut v = ok.clone();
        v[5] = 9;
        assert_eq!(AudioFrame::decode(&v), Err(FrameError::Codec(9)));
        for channels in [0, 3] {
            let mut v = ok.clone();
            v[6] = channels;
            assert_eq!(AudioFrame::decode(&v), Err(FrameError::Channels(channels)));
        }
    }
}
