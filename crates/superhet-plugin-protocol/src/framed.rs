//! Framed transport: length-prefixed frames on a Unix domain socket.
//!
//! Wire format (all integers little-endian): `u32 payload_len | u8 kind | payload`. A frame
//! whose payload exceeds [`MAX_PAYLOAD`] or whose kind is unknown is a protocol violation and
//! ends the session. The first frame in each direction is a `hello`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Largest payload of one frame (bytes).
pub const MAX_PAYLOAD: usize = 4 * 1024 * 1024;

/// Bytes before the payload.
pub const HEADER_LEN: usize = 5;

/// Largest `status` document (bytes); larger ones are dropped by the host.
pub const MAX_STATUS: usize = 256 * 1024;

/// Largest `media` object (bytes, without the header); larger ones are dropped by the host.
pub const MAX_MEDIA: usize = 256 * 1024;

/// Most distinct media keys the host keeps per instance.
pub const MAX_MEDIA_KEYS: usize = 16;

/// What a frame carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum FrameKind {
    /// JSON handshake ([`HostHello`] from the host, [`PluginHello`] from the plugin).
    Hello = 1,
    /// Host to plugin: sample block, [`SamplesHeader`] + raw samples in `input.format`.
    Samples = 2,
    /// Host to plugin: JSON control message (config change, flush).
    Control = 3,
    /// Plugin to host: JSON event, same envelope as a simple-transport line.
    Event = 4,
    /// Plugin to host: PCM s16le interleaved audio at `output.audio`.
    Audio = 5,
    /// Plugin to host: [`VoiceFrameHeader`] + codec bytes.
    VoiceFrame = 6,
    /// Plugin to host: UTF-8 log line.
    Log = 7,
    /// Either direction: liveness, empty payload.
    Heartbeat = 8,
    /// Plugin to host: JSON object, the plugin's complete current state (replaces the previous
    /// one; the host keeps the latest and serves it through the control API `status`).
    Status = 9,
    /// Plugin to host: [`MediaHeader`] + bytes of one binary object (a slideshow image); the
    /// host keeps the latest per key and serves it through the control API `media`.
    Media = 10,
    /// Plugin to host: JSON [`TuneRequest`], retune the claimed window (exclusive plugins that
    /// declare a centre frequency range).
    Tune = 11,
}

impl FrameKind {
    /// Decode the kind byte.
    #[must_use]
    pub fn from_u8(byte: u8) -> Option<Self> {
        Some(match byte {
            1 => Self::Hello,
            2 => Self::Samples,
            3 => Self::Control,
            4 => Self::Event,
            5 => Self::Audio,
            6 => Self::VoiceFrame,
            7 => Self::Log,
            8 => Self::Heartbeat,
            9 => Self::Status,
            10 => Self::Media,
            11 => Self::Tune,
            _ => return None,
        })
    }
}

/// A frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Kind.
    pub kind: FrameKind,
    /// Payload.
    pub payload: Vec<u8>,
}

/// Framing violation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// Declared payload is larger than [`MAX_PAYLOAD`].
    #[error("frame payload of {0} bytes exceeds the limit")]
    TooLarge(usize),
    /// Unknown kind byte.
    #[error("unknown frame kind {0}")]
    UnknownKind(u8),
    /// A payload is shorter than its fixed header.
    #[error("payload too short for {0}")]
    Truncated(&'static str),
}

/// Encode a frame.
///
/// # Errors
/// [`FrameError::TooLarge`] when the payload exceeds [`MAX_PAYLOAD`].
pub fn encode(kind: FrameKind, payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    if payload.len() > MAX_PAYLOAD {
        return Err(FrameError::TooLarge(payload.len()));
    }
    let len = u32::try_from(payload.len()).map_err(|_| FrameError::TooLarge(payload.len()))?;
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.push(kind as u8);
    out.extend_from_slice(payload);
    Ok(out)
}

/// Incremental decoder: feed bytes as they arrive, pull complete frames.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    /// Empty decoder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Append received bytes.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next complete frame, if one is buffered.
    ///
    /// # Errors
    /// A violation; the stream is unusable afterwards.
    pub fn next_frame(&mut self) -> Result<Option<Frame>, FrameError> {
        if self.buf.len() < HEADER_LEN {
            return Ok(None);
        }
        let len = u32::from_le_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if len > MAX_PAYLOAD {
            return Err(FrameError::TooLarge(len));
        }
        let kind = FrameKind::from_u8(self.buf[4]).ok_or(FrameError::UnknownKind(self.buf[4]))?;
        if self.buf.len() < HEADER_LEN + len {
            return Ok(None);
        }
        let payload = self.buf[HEADER_LEN..HEADER_LEN + len].to_vec();
        self.buf.drain(..HEADER_LEN + len);
        Ok(Some(Frame { kind, payload }))
    }
}

/// Host handshake (`hello` host to plugin).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostHello {
    /// Contract version of the host.
    pub protocol: u32,
    /// Plugin id.
    pub plugin_id: String,
    /// Instance id.
    pub instance_id: String,
    /// Device the instance is bound to.
    #[serde(default)]
    pub device: Option<String>,
    /// Centre frequency (Hz) of the instance.
    #[serde(default)]
    pub freq_hz: Option<u64>,
    /// Instance configuration chosen by the operator (free-form object or null).
    #[serde(default)]
    pub config: Value,
}

/// Plugin handshake (`hello` plugin to host).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PluginHello {
    /// Contract version the plugin speaks; must equal the host's.
    pub protocol: u32,
    /// Optional feature names the plugin supports (informational in v1).
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// Header of a `samples` payload (20 bytes), followed by the raw samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SamplesHeader {
    /// Block counter; gaps mean dropped blocks.
    pub seq: u64,
    /// Capture time of the first sample (Unix nanoseconds).
    pub timestamp_ns: u64,
    /// Tuning epoch of the device when captured.
    pub tuning_epoch: u32,
}

impl SamplesHeader {
    /// Encoded size.
    pub const LEN: usize = 20;

    /// Prepend the header to the samples.
    #[must_use]
    pub fn encode(&self, samples: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(Self::LEN + samples.len());
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&self.timestamp_ns.to_le_bytes());
        out.extend_from_slice(&self.tuning_epoch.to_le_bytes());
        out.extend_from_slice(samples);
        out
    }

    /// Split a payload into header and samples.
    ///
    /// # Errors
    /// [`FrameError::Truncated`] when the payload is shorter than the header.
    pub fn decode(payload: &[u8]) -> Result<(Self, &[u8]), FrameError> {
        let head = payload
            .get(..Self::LEN)
            .ok_or(FrameError::Truncated("samples"))?;
        let seq = read_u64(&head[0..8]);
        let timestamp_ns = read_u64(&head[8..16]);
        let tuning_epoch = u32::from_le_bytes([head[16], head[17], head[18], head[19]]);
        Ok((
            Self {
                seq,
                timestamp_ns,
                tuning_epoch,
            },
            &payload[Self::LEN..],
        ))
    }
}

/// Payload of a `tune` frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TuneRequest {
    /// New centre frequency (Hz); must lie inside the manifest's `device.center_hz_min..=max`.
    pub freq_hz: u64,
}

/// Header of a `media` payload: `u8 key_len | key | u8 mime_len | mime | bytes`.
///
/// The key (`[a-z0-9._-]`, at most 64 characters) names the object (for example `slide.c224`);
/// the host stores the latest bytes per key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaHeader {
    /// Object key.
    pub key: String,
    /// MIME type of the bytes (`image/jpeg`).
    pub mime: String,
}

impl MediaHeader {
    /// Whether `key` is acceptable (1..=64 characters of `[a-z0-9._-]`).
    #[must_use]
    pub fn valid_key(key: &str) -> bool {
        (1..=64).contains(&key.len())
            && key.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
            })
    }

    /// Prepend the header to the object bytes; `None` when key or MIME type is too long.
    #[must_use]
    pub fn encode(&self, bytes: &[u8]) -> Option<Vec<u8>> {
        let key_len = u8::try_from(self.key.len()).ok()?;
        let mime_len = u8::try_from(self.mime.len()).ok()?;
        let mut out = Vec::with_capacity(2 + self.key.len() + self.mime.len() + bytes.len());
        out.push(key_len);
        out.extend_from_slice(self.key.as_bytes());
        out.push(mime_len);
        out.extend_from_slice(self.mime.as_bytes());
        out.extend_from_slice(bytes);
        Some(out)
    }

    /// Split a payload into header and object bytes.
    ///
    /// # Errors
    /// [`FrameError::Truncated`] when the payload is too short or key or MIME type are not UTF-8.
    pub fn decode(payload: &[u8]) -> Result<(Self, &[u8]), FrameError> {
        const WHAT: &str = "media";
        let field = |from: usize| -> Result<(String, usize), FrameError> {
            let len = usize::from(*payload.get(from).ok_or(FrameError::Truncated(WHAT))?);
            let text = payload
                .get(from + 1..from + 1 + len)
                .and_then(|raw| std::str::from_utf8(raw).ok())
                .ok_or(FrameError::Truncated(WHAT))?;
            Ok((text.to_owned(), from + 1 + len))
        };
        let (key, next) = field(0)?;
        let (mime, next) = field(next)?;
        Ok((Self { key, mime }, &payload[next..]))
    }
}

/// Header of a `voice_frame` payload: `u8 codec_len | codec | u64 call_id | u64 ts_ns | bytes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceFrameHeader {
    /// Codec id (`tetra-acelp`, ...).
    pub codec: String,
    /// Call the frame belongs to.
    pub call_id: u64,
    /// Time of the frame (Unix nanoseconds).
    pub timestamp_ns: u64,
}

impl VoiceFrameHeader {
    /// Prepend the header to the codec bytes; `None` when the codec id is longer than 255 bytes.
    #[must_use]
    pub fn encode(&self, bytes: &[u8]) -> Option<Vec<u8>> {
        let len = u8::try_from(self.codec.len()).ok()?;
        let mut out = Vec::with_capacity(18 + self.codec.len() + bytes.len());
        out.push(len);
        out.extend_from_slice(self.codec.as_bytes());
        out.extend_from_slice(&self.call_id.to_le_bytes());
        out.extend_from_slice(&self.timestamp_ns.to_le_bytes());
        out.extend_from_slice(bytes);
        Some(out)
    }

    /// Split a payload into header and codec bytes.
    ///
    /// # Errors
    /// [`FrameError::Truncated`] when the payload is too short or the codec id is not UTF-8.
    pub fn decode(payload: &[u8]) -> Result<(Self, &[u8]), FrameError> {
        const WHAT: &str = "voice frame";
        let len = usize::from(*payload.first().ok_or(FrameError::Truncated(WHAT))?);
        let codec = payload
            .get(1..1 + len)
            .and_then(|raw| std::str::from_utf8(raw).ok())
            .ok_or(FrameError::Truncated(WHAT))?;
        let rest = payload
            .get(1 + len..)
            .filter(|rest| rest.len() >= 16)
            .ok_or(FrameError::Truncated(WHAT))?;
        Ok((
            Self {
                codec: codec.to_owned(),
                call_id: read_u64(&rest[0..8]),
                timestamp_ns: read_u64(&rest[8..16]),
            },
            &rest[16..],
        ))
    }
}

fn read_u64(bytes: &[u8]) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[..8]);
    u64::from_le_bytes(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_across_split_reads() {
        let mut wire = encode(FrameKind::Event, b"{\"a\":1}").expect("encode");
        wire.extend(encode(FrameKind::Heartbeat, b"").expect("encode"));
        let mut dec = FrameDecoder::new();
        let mut frames = Vec::new();
        for byte in &wire {
            dec.push(std::slice::from_ref(byte));
            while let Some(f) = dec.next_frame().expect("valid") {
                frames.push(f);
            }
        }
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].kind, FrameKind::Event);
        assert_eq!(frames[0].payload, b"{\"a\":1}");
        assert_eq!(frames[1].kind, FrameKind::Heartbeat);
    }

    #[test]
    fn rejects_violations() {
        let mut dec = FrameDecoder::new();
        dec.push(&[0xff, 0xff, 0xff, 0x7f, 4]);
        assert!(matches!(dec.next_frame(), Err(FrameError::TooLarge(_))));
        let mut dec = FrameDecoder::new();
        dec.push(&[0, 0, 0, 0, 99]);
        assert_eq!(dec.next_frame(), Err(FrameError::UnknownKind(99)));
        assert!(encode(FrameKind::Audio, &vec![0; MAX_PAYLOAD + 1]).is_err());
    }

    #[test]
    fn headers_round_trip() {
        let head = SamplesHeader {
            seq: 7,
            timestamp_ns: 123_456_789,
            tuning_epoch: 3,
        };
        let payload = head.encode(&[1, 2, 3]);
        let (got, rest) = SamplesHeader::decode(&payload).expect("decode");
        assert_eq!((got, rest), (head, &[1u8, 2, 3][..]));
        assert!(SamplesHeader::decode(&payload[..10]).is_err());

        let voice = VoiceFrameHeader {
            codec: "tetra-acelp".into(),
            call_id: 42,
            timestamp_ns: 9,
        };
        let payload = voice.encode(&[0xaa, 0xbb]).expect("encode");
        let (got, rest) = VoiceFrameHeader::decode(&payload).expect("decode");
        assert_eq!((got, rest), (voice, &[0xaau8, 0xbb][..]));
        assert!(VoiceFrameHeader::decode(&payload[..8]).is_err());
    }

    #[test]
    fn media_header_round_trips() {
        let head = MediaHeader {
            key: "slide.c224".into(),
            mime: "image/jpeg".into(),
        };
        let payload = head.encode(&[0xff, 0xd8]).expect("encode");
        let (got, rest) = MediaHeader::decode(&payload).expect("decode");
        assert_eq!((got, rest), (head, &[0xffu8, 0xd8][..]));
        assert!(MediaHeader::decode(&payload[..5]).is_err());
        assert!(MediaHeader::decode(&[]).is_err());
        assert!(MediaHeader::valid_key("slide.c224") && !MediaHeader::valid_key("Slide"));
        assert!(!MediaHeader::valid_key("") && !MediaHeader::valid_key(&"a".repeat(65)));
    }

    #[test]
    fn new_kinds_decode() {
        for kind in [FrameKind::Status, FrameKind::Media, FrameKind::Tune] {
            assert_eq!(FrameKind::from_u8(kind as u8), Some(kind));
        }
    }
}
