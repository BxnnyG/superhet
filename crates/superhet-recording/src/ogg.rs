//! Opus in Ogg (RFC 7845) without any re-encoding: the 20 ms Opus packets the daemon already
//! publishes are wrapped into Ogg pages as they are.
//!
//! [`OggOpusWriter`] is sans-I/O: it returns the bytes of finished pages, the caller appends them
//! to a file or a buffer. [`parse`] reads a stream back and checks every page CRC; it is used by
//! the tests and by callers that want to validate a file.

use std::fmt;

/// Samples per channel of one 20 ms packet at 48 kHz.
pub const PACKET_SAMPLES: u32 = 960;
/// Decoder pre-skip in samples: the look-ahead of libopus at 48 kHz.
pub const PRE_SKIP: u16 = 312;
/// Pages are flushed after this many packets (1 s of audio).
const PACKETS_PER_PAGE: usize = 50;
/// Most lacing segments in one page.
const MAX_SEGMENTS: usize = 255;
/// Largest comment header the writer emits (it has to fit one page).
const MAX_TAGS_BYTES: usize = 32 * 1024;
const VENDOR: &str = "Superhet";

const FLAG_CONTINUED: u8 = 0x01;
const FLAG_BOS: u8 = 0x02;
const FLAG_EOS: u8 = 0x04;

const fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let mut r = (i as u32) << 24;
        let mut bit = 0;
        while bit < 8 {
            r = if r & 0x8000_0000 != 0 {
                (r << 1) ^ 0x04C1_1DB7
            } else {
                r << 1
            };
            bit += 1;
        }
        table[i] = r;
        i += 1;
    }
    table
}

static CRC_TABLE: [u32; 256] = crc_table();

/// The Ogg page checksum: CRC-32 with polynomial 0x04C11DB7, no reflection, initial value 0,
/// no final xor.
#[must_use]
pub fn crc32(data: &[u8]) -> u32 {
    data.iter().fold(0u32, |crc, &byte| {
        (crc << 8) ^ CRC_TABLE[usize::from((crc >> 24) as u8 ^ byte)]
    })
}

/// Comment header entries (`KEY=value`); the vendor string is always `Superhet`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpusTags {
    /// `(key, value)` pairs, keys are upper-case ASCII without `=`.
    pub comments: Vec<(String, String)>,
}

impl OpusTags {
    /// Add one comment; keys with characters outside `A-Z0-9_` (after upper-casing) and empty
    /// values are dropped.
    pub fn add(&mut self, key: &str, value: &str) {
        let key = key.to_ascii_uppercase();
        if key.is_empty()
            || !key
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
            || value.is_empty()
        {
            return;
        }
        self.comments.push((key, value.to_owned()));
    }

    fn encode(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"OpusTags");
        out.extend_from_slice(&(VENDOR.len() as u32).to_le_bytes());
        out.extend_from_slice(VENDOR.as_bytes());
        let mut count = 0u32;
        let mut entries = Vec::new();
        for (key, value) in &self.comments {
            let entry = format!("{key}={value}");
            if out.len() + entries.len() + entry.len() + 8 > MAX_TAGS_BYTES {
                break;
            }
            entries.extend_from_slice(&(entry.len() as u32).to_le_bytes());
            entries.extend_from_slice(entry.as_bytes());
            count += 1;
        }
        out.extend_from_slice(&count.to_le_bytes());
        out.extend_from_slice(&entries);
        out
    }
}

fn lacing(len: usize) -> impl Iterator<Item = u8> {
    // A packet of n * 255 bytes ends with a zero segment.
    (0..len / 255)
        .map(|_| 255u8)
        .chain(std::iter::once((len % 255) as u8))
}

fn page(flags: u8, granule: u64, serial: u32, seq: u32, segments: &[u8], body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(27 + segments.len() + body.len());
    out.extend_from_slice(b"OggS");
    out.push(0);
    out.push(flags);
    out.extend_from_slice(&granule.to_le_bytes());
    out.extend_from_slice(&serial.to_le_bytes());
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.push(segments.len() as u8);
    out.extend_from_slice(segments);
    out.extend_from_slice(body);
    let crc = crc32(&out);
    out[22..26].copy_from_slice(&crc.to_le_bytes());
    out
}

/// Writes one logical Opus stream.
pub struct OggOpusWriter {
    serial: u32,
    seq: u32,
    /// Samples of audio written so far (without the pre-skip).
    samples: u64,
    segments: Vec<u8>,
    body: Vec<u8>,
    packets: usize,
}

impl OggOpusWriter {
    /// Start a stream; returns the writer and the two header pages (identification and
    /// comments) that must be written first.
    #[must_use]
    pub fn new(serial: u32, channels: u8, tags: &OpusTags) -> (Self, Vec<u8>) {
        let mut head = Vec::with_capacity(19);
        head.extend_from_slice(b"OpusHead");
        head.push(1);
        head.push(channels.clamp(1, 2));
        head.extend_from_slice(&PRE_SKIP.to_le_bytes());
        head.extend_from_slice(&48_000u32.to_le_bytes());
        head.extend_from_slice(&0i16.to_le_bytes());
        head.push(0);
        let mut out = page(
            FLAG_BOS,
            0,
            serial,
            0,
            &lacing(head.len()).collect::<Vec<_>>(),
            &head,
        );
        let comments = tags.encode();
        out.extend(page(
            0,
            0,
            serial,
            1,
            &lacing(comments.len()).collect::<Vec<_>>(),
            &comments,
        ));
        (
            Self {
                serial,
                seq: 2,
                samples: 0,
                segments: Vec::new(),
                body: Vec::new(),
                packets: 0,
            },
            out,
        )
    }

    /// Append one packet of `samples` samples per channel (960 for the daemon's 20 ms frames).
    /// Returns the bytes of the pages this packet completed, if any.
    pub fn push(&mut self, packet: &[u8], samples: u32) -> Option<Vec<u8>> {
        let needed = packet.len() / 255 + 1;
        let mut flushed = None;
        if self.segments.len() + needed > MAX_SEGMENTS {
            flushed = self.take_page(0);
        }
        self.segments.extend(lacing(packet.len()));
        self.body.extend_from_slice(packet);
        self.packets += 1;
        self.samples += u64::from(samples);
        if self.packets >= PACKETS_PER_PAGE {
            let page = self.take_page(0);
            return match (flushed, page) {
                (Some(mut a), Some(b)) => {
                    a.extend(b);
                    Some(a)
                }
                (a, b) => a.or(b),
            };
        }
        flushed
    }

    /// Samples per channel written so far.
    #[must_use]
    pub fn samples(&self) -> u64 {
        self.samples
    }

    /// Emit the open page now (it may be short).
    pub fn flush(&mut self) -> Option<Vec<u8>> {
        self.take_page(0)
    }

    /// Close the stream: the last page carries the end-of-stream flag (an empty one if nothing
    /// was pending).
    #[must_use]
    pub fn finish(mut self) -> Vec<u8> {
        self.take_page(FLAG_EOS).unwrap_or_default()
    }

    fn take_page(&mut self, flags: u8) -> Option<Vec<u8>> {
        if self.segments.is_empty() && flags == 0 {
            return None;
        }
        let granule = self.samples + u64::from(PRE_SKIP);
        let out = page(
            flags,
            granule,
            self.serial,
            self.seq,
            &self.segments,
            &self.body,
        );
        self.seq += 1;
        self.segments.clear();
        self.body.clear();
        self.packets = 0;
        Some(out)
    }
}

/// Why a stream is not valid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    /// Missing `OggS` or truncated page.
    Truncated,
    /// Unsupported stream structure version.
    Version,
    /// The checksum of the page with this sequence number does not match.
    Crc(u32),
    /// Pages of different logical streams, or a gap in the page sequence numbers.
    Sequence,
    /// Not an Opus stream (headers wrong).
    NotOpus,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("truncated or not an Ogg stream"),
            Self::Version => f.write_str("unsupported Ogg version"),
            Self::Crc(n) => write!(f, "bad CRC on page {n}"),
            Self::Sequence => f.write_str("page sequence broken"),
            Self::NotOpus => f.write_str("not an Opus stream"),
        }
    }
}

impl std::error::Error for ParseError {}

/// One parsed page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page {
    /// Header type flags (1 continued, 2 first page, 4 last page).
    pub flags: u8,
    /// Granule position.
    pub granule: u64,
    /// Logical stream serial number.
    pub serial: u32,
    /// Page sequence number.
    pub seq: u32,
    /// Complete packets of the page (continuation across pages is not supported).
    pub packets: Vec<Vec<u8>>,
}

/// A parsed, validated Opus file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpusFile {
    /// Channel count from `OpusHead`.
    pub channels: u8,
    /// Pre-skip from `OpusHead`.
    pub pre_skip: u16,
    /// `(key, value)` comment pairs.
    pub comments: Vec<(String, String)>,
    /// Audio packets.
    pub packets: Vec<Vec<u8>>,
    /// Granule position of the last page.
    pub last_granule: u64,
    /// The last page carries the end-of-stream flag.
    pub ended: bool,
    /// Number of pages.
    pub pages: usize,
}

impl OpusFile {
    /// Duration in milliseconds from the last granule position (pre-skip removed).
    #[must_use]
    pub fn duration_ms(&self) -> u64 {
        self.last_granule.saturating_sub(u64::from(self.pre_skip)) * 1000 / 48_000
    }
}

/// Split a byte stream into pages and verify structure and CRCs.
///
/// # Errors
/// [`ParseError`] for the first problem found.
pub fn parse_pages(mut data: &[u8]) -> Result<Vec<Page>, ParseError> {
    let mut pages = Vec::new();
    while !data.is_empty() {
        if data.len() < 27 || &data[..4] != b"OggS" {
            return Err(ParseError::Truncated);
        }
        if data[4] != 0 {
            return Err(ParseError::Version);
        }
        let count = usize::from(data[26]);
        let table = data.get(27..27 + count).ok_or(ParseError::Truncated)?;
        let body_len: usize = table.iter().map(|&s| usize::from(s)).sum();
        let total = 27 + count + body_len;
        let raw = data.get(..total).ok_or(ParseError::Truncated)?;
        let stored = u32::from_le_bytes([raw[22], raw[23], raw[24], raw[25]]);
        let mut copy = raw.to_vec();
        copy[22..26].fill(0);
        let seq = u32::from_le_bytes([raw[18], raw[19], raw[20], raw[21]]);
        if crc32(&copy) != stored {
            return Err(ParseError::Crc(seq));
        }
        let mut packets = Vec::new();
        let mut current = Vec::new();
        let mut at = 27 + count;
        for &lace in table {
            current.extend_from_slice(&raw[at..at + usize::from(lace)]);
            at += usize::from(lace);
            if lace < 255 {
                packets.push(std::mem::take(&mut current));
            }
        }
        if !current.is_empty() {
            return Err(ParseError::Truncated);
        }
        pages.push(Page {
            flags: raw[5],
            granule: u64::from_le_bytes([
                raw[6], raw[7], raw[8], raw[9], raw[10], raw[11], raw[12], raw[13],
            ]),
            serial: u32::from_le_bytes([raw[14], raw[15], raw[16], raw[17]]),
            seq,
            packets,
        });
        data = &data[total..];
    }
    Ok(pages)
}

/// Parse and validate a complete Opus-in-Ogg file (one logical stream).
///
/// # Errors
/// [`ParseError`] for the first problem found.
pub fn parse(data: &[u8]) -> Result<OpusFile, ParseError> {
    let pages = parse_pages(data)?;
    let first = pages.first().ok_or(ParseError::Truncated)?;
    if first.flags & FLAG_BOS == 0 || first.flags & FLAG_CONTINUED != 0 {
        return Err(ParseError::NotOpus);
    }
    for (expected, page) in (0u32..).zip(&pages) {
        if page.serial != first.serial || page.seq != expected {
            return Err(ParseError::Sequence);
        }
    }
    let head = first.packets.first().ok_or(ParseError::NotOpus)?;
    if head.len() < 19 || &head[..8] != b"OpusHead" || head[8] != 1 {
        return Err(ParseError::NotOpus);
    }
    let channels = head[9];
    let pre_skip = u16::from_le_bytes([head[10], head[11]]);
    let tags = pages
        .get(1)
        .and_then(|p| p.packets.first())
        .ok_or(ParseError::NotOpus)?;
    let comments = parse_tags(tags).ok_or(ParseError::NotOpus)?;
    let packets: Vec<Vec<u8>> = pages
        .iter()
        .skip(2)
        .flat_map(|p| p.packets.iter().cloned())
        .collect();
    let last = pages.last().ok_or(ParseError::Truncated)?;
    Ok(OpusFile {
        channels,
        pre_skip,
        comments,
        packets,
        last_granule: last.granule,
        ended: last.flags & FLAG_EOS != 0,
        pages: pages.len(),
    })
}

fn parse_tags(data: &[u8]) -> Option<Vec<(String, String)>> {
    let rest = data.strip_prefix(b"OpusTags")?;
    let u32_at = |at: usize| -> Option<usize> {
        let b = rest.get(at..at + 4)?;
        usize::try_from(u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok()
    };
    let vendor = u32_at(0)?;
    let mut at = 4 + vendor;
    rest.get(4..at)?;
    let count = u32_at(at)?;
    at += 4;
    let mut out = Vec::new();
    for _ in 0..count {
        let len = u32_at(at)?;
        at += 4;
        let entry = std::str::from_utf8(rest.get(at..at + len)?).ok()?;
        at += len;
        let (k, v) = entry.split_once('=')?;
        out.push((k.to_owned(), v.to_owned()));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bit-by-bit reference implementation of the Ogg checksum.
    fn crc_reference(data: &[u8]) -> u32 {
        let mut crc = 0u32;
        for &byte in data {
            crc ^= u32::from(byte) << 24;
            for _ in 0..8 {
                crc = if crc & 0x8000_0000 != 0 {
                    (crc << 1) ^ 0x04C1_1DB7
                } else {
                    crc << 1
                };
            }
        }
        crc
    }

    #[test]
    fn crc_matches_the_bitwise_reference() {
        for data in [&b""[..], b"123456789", b"OggS\0\x02", &[0xFF; 300]] {
            assert_eq!(crc32(data), crc_reference(data));
        }
    }

    fn sample_file(packets: &[Vec<u8>]) -> Vec<u8> {
        let mut tags = OpusTags::default();
        tags.add("title", "Test");
        tags.add("bad key", "x");
        tags.add("FREQUENCY", "145500000");
        let (mut writer, mut bytes) = OggOpusWriter::new(0xDEAD_BEEF, 1, &tags);
        for packet in packets {
            if let Some(page) = writer.push(packet, PACKET_SAMPLES) {
                bytes.extend(page);
            }
        }
        bytes.extend(writer.finish());
        bytes
    }

    #[test]
    fn files_validate_and_round_trip() {
        // Odd sizes, an exact multiple of 255 and an empty packet exercise the lacing.
        let packets: Vec<Vec<u8>> = (0..180usize)
            .map(|i| match i % 4 {
                0 => vec![i as u8; 20 + i],
                1 => vec![1; 255],
                2 => vec![2; 510],
                _ => vec![],
            })
            .collect();
        let bytes = sample_file(&packets);
        let file = parse(&bytes).expect("valid file");
        assert_eq!(file.channels, 1);
        assert_eq!(file.pre_skip, PRE_SKIP);
        assert_eq!(file.packets, packets);
        assert!(file.ended);
        // 180 packets * 20 ms.
        assert_eq!(file.duration_ms(), 3600);
        assert_eq!(
            file.comments,
            vec![
                ("TITLE".into(), "Test".into()),
                ("FREQUENCY".into(), "145500000".into())
            ]
        );
        assert!(
            file.pages >= 2 + 3 + 1,
            "50 packets per page: {}",
            file.pages
        );
    }

    #[test]
    fn page_headers_follow_the_spec() {
        let bytes = sample_file(&[vec![9; 40]]);
        let pages = parse_pages(&bytes).expect("pages");
        assert_eq!(pages[0].flags, FLAG_BOS);
        assert_eq!(pages[0].granule, 0);
        assert_eq!(pages[1].granule, 0);
        assert_eq!(pages[0].packets[0].len(), 19);
        let last = pages.last().expect("last");
        assert_eq!(last.flags & FLAG_EOS, FLAG_EOS);
        assert_eq!(last.granule, u64::from(PRE_SKIP) + 960);
        assert!(pages.iter().all(|p| p.serial == 0xDEAD_BEEF));
    }

    #[test]
    fn a_stream_without_packets_still_ends_properly() {
        // An empty end-of-stream page closes a stream that has no pending packets.
        let bytes = sample_file(&[]);
        let file = parse(&bytes).expect("valid");
        assert!(file.ended);
        assert!(file.packets.is_empty());
        assert_eq!(file.duration_ms(), 0);
    }

    #[test]
    fn corruption_is_detected() {
        let mut bytes = sample_file(&[vec![7; 30], vec![8; 30]]);
        let last = bytes.len() - 5;
        bytes[last] ^= 0x40;
        assert!(matches!(parse(&bytes), Err(ParseError::Crc(_))));
        let bytes = sample_file(&[vec![7; 30]]);
        assert_eq!(parse(&bytes[..bytes.len() - 3]), Err(ParseError::Truncated));
        assert_eq!(
            parse(b"RIFF.............................."),
            Err(ParseError::Truncated)
        );
    }

    #[test]
    fn many_small_packets_never_overflow_the_segment_table() {
        let packets = vec![vec![]; 300];
        let file = parse(&sample_file(&packets)).expect("valid");
        assert_eq!(file.packets.len(), 300);
    }

    #[test]
    fn a_long_run_of_large_packets_splits_on_the_segment_limit() {
        // 40 packets of 3000 bytes need 12 segments each: at most 21 fit one page.
        let packets: Vec<Vec<u8>> = (0..40).map(|_| vec![5; 3000]).collect();
        let bytes = sample_file(&packets);
        let pages = parse_pages(&bytes).expect("pages");
        assert!(pages.iter().all(|p| p.packets.len() <= 21));
        assert_eq!(parse(&bytes).expect("valid").packets, packets);
    }
}
