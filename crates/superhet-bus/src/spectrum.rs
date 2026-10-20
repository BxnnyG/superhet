//! Binary spectrum frame published on `sh.v1.dev.<dev>.spectrum`.
//!
//! Wire format version 1, all integers and floats little-endian, 32-byte header:
//!
//! | offset | type | field |
//! |---|---|---|
//! | 0 | `u8` | `version` (= 1) |
//! | 1 | `u8` | `format` (= 0: one `u8` per bin, linear in dB) |
//! | 2 | `u16` | `bin_count` |
//! | 4 | `u32` | `sample_rate` in Hz (the frame spans `center ± sample_rate / 2`) |
//! | 8 | `u64` | `center_hz` |
//! | 16 | `u64` | `timestamp_us`, Unix time in microseconds at frame completion |
//! | 24 | `f32` | `min_db` (dBFS mapped to 0) |
//! | 28 | `f32` | `max_db` (dBFS mapped to 255) |
//! | 32 | `u8[bin_count]` | bins, lowest frequency first (`center - sample_rate / 2`) |
//!
//! Quantisation: `q = round((db - min_db) / (max_db - min_db) * 255)` clamped to 0..=255;
//! `db = min_db + q / 255 * (max_db - min_db)`. Bin `i` is centred on
//! `center - sample_rate/2 + (i + 0.5) * sample_rate / bin_count` (approximately; the DC bin of
//! the underlying FFT sits at index `bin_count / 2`).
//!
//! Spectrum is lossy live data: it is published on core NATS (no JetStream) and may be dropped.

/// Current wire version.
pub const VERSION: u8 = 1;
/// Size of the fixed header in bytes.
pub const HEADER_LEN: usize = 32;
/// Format id: one `u8` per bin, linear in dB between `min_db` and `max_db`.
pub const FORMAT_U8_DB: u8 = 0;

/// Decoding failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    /// Shorter than the header, or the body length does not match `bin_count`.
    #[error("spectrum frame has wrong length")]
    Length,
    /// Unknown wire version.
    #[error("unsupported spectrum frame version {0}")]
    Version(u8),
    /// Unknown bin format.
    #[error("unsupported spectrum bin format {0}")]
    Format(u8),
}

/// A quantised spectrum frame.
#[derive(Debug, Clone, PartialEq)]
pub struct SpectrumFrame {
    /// Centre frequency in Hz.
    pub center_hz: u64,
    /// Sample rate (= spanned bandwidth) in Hz.
    pub sample_rate: u32,
    /// Unix timestamp in microseconds.
    pub timestamp_us: u64,
    /// dBFS value mapped to quantised 0.
    pub min_db: f32,
    /// dBFS value mapped to quantised 255.
    pub max_db: f32,
    /// Quantised bins, lowest frequency first.
    pub bins: Vec<u8>,
}

impl SpectrumFrame {
    /// Quantise dBFS values into a frame. Values outside `min_db..=max_db` are clamped;
    /// `NaN` maps to 0. At most `u16::MAX` bins are kept.
    pub fn quantize(
        center_hz: u64,
        sample_rate: u32,
        timestamp_us: u64,
        min_db: f32,
        max_db: f32,
        db: &[f32],
    ) -> Self {
        let span = (max_db - min_db).max(f32::EPSILON);
        let bins = db
            .iter()
            .take(usize::from(u16::MAX))
            .map(|v| (((v - min_db) / span) * 255.0).round().clamp(0.0, 255.0) as u8)
            .collect();
        Self {
            center_hz,
            sample_rate,
            timestamp_us,
            min_db,
            max_db,
            bins,
        }
    }

    /// Dequantise bin `i` to dBFS.
    pub fn db(&self, i: usize) -> Option<f32> {
        self.bins
            .get(i)
            .map(|q| self.min_db + f32::from(*q) / 255.0 * (self.max_db - self.min_db))
    }

    /// Serialise to the wire format. Bins beyond `u16::MAX` are not encoded.
    pub fn encode(&self) -> Vec<u8> {
        let n = self.bins.len().min(usize::from(u16::MAX));
        let mut out = Vec::with_capacity(HEADER_LEN + n);
        out.push(VERSION);
        out.push(FORMAT_U8_DB);
        out.extend_from_slice(&(n as u16).to_le_bytes());
        out.extend_from_slice(&self.sample_rate.to_le_bytes());
        out.extend_from_slice(&self.center_hz.to_le_bytes());
        out.extend_from_slice(&self.timestamp_us.to_le_bytes());
        out.extend_from_slice(&self.min_db.to_le_bytes());
        out.extend_from_slice(&self.max_db.to_le_bytes());
        out.extend_from_slice(&self.bins[..n]);
        out
    }

    /// Parse the wire format.
    ///
    /// # Errors
    /// Fails on truncated data, trailing bytes, or unknown version/format.
    pub fn decode(data: &[u8]) -> Result<Self, FrameError> {
        let header: &[u8; HEADER_LEN] = data
            .get(..HEADER_LEN)
            .and_then(|h| h.try_into().ok())
            .ok_or(FrameError::Length)?;
        if header[0] != VERSION {
            return Err(FrameError::Version(header[0]));
        }
        if header[1] != FORMAT_U8_DB {
            return Err(FrameError::Format(header[1]));
        }
        let bin_count = usize::from(u16::from_le_bytes([header[2], header[3]]));
        if data.len() != HEADER_LEN + bin_count {
            return Err(FrameError::Length);
        }
        // Slices below have fixed lengths, so the conversions cannot fail.
        let u32_at =
            |o: usize| u32::from_le_bytes([header[o], header[o + 1], header[o + 2], header[o + 3]]);
        let u64_at = |o: usize| {
            let mut b = [0u8; 8];
            b.copy_from_slice(&header[o..o + 8]);
            u64::from_le_bytes(b)
        };
        Ok(Self {
            sample_rate: u32_at(4),
            center_hz: u64_at(8),
            timestamp_us: u64_at(16),
            min_db: f32::from_bits(u32_at(24)),
            max_db: f32::from_bits(u32_at(28)),
            bins: data[HEADER_LEN..].to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let db: Vec<f32> = (0..1024).map(|i| -120.0 + i as f32 * 0.1).collect();
        let f = SpectrumFrame::quantize(
            433_920_000,
            2_048_000,
            1_700_000_000_123_456,
            -120.0,
            0.0,
            &db,
        );
        let bytes = f.encode();
        assert_eq!(bytes.len(), HEADER_LEN + 1024);
        assert_eq!(bytes[0], VERSION);
        assert_eq!(SpectrumFrame::decode(&bytes), Ok(f));
    }

    #[test]
    fn quantisation_maps_and_clamps() {
        let f = SpectrumFrame::quantize(
            0,
            1,
            0,
            -100.0,
            0.0,
            &[-200.0, -100.0, -50.0, 0.0, 30.0, f32::NAN],
        );
        assert_eq!(f.bins, vec![0, 0, 128, 255, 255, 0]);
        assert!((f.db(2).expect("bin") + 49.8).abs() < 0.1);
        assert_eq!(f.db(99), None);
    }

    #[test]
    fn quantisation_error_is_below_half_step() {
        let step = 120.0 / 255.0;
        for i in 0..=1200 {
            let v = -120.0 + i as f32 * 0.1;
            let f = SpectrumFrame::quantize(0, 1, 0, -120.0, 0.0, &[v]);
            assert!((f.db(0).expect("bin") - v).abs() <= step / 2.0 + 1e-3);
        }
    }

    #[test]
    fn rejects_malformed() {
        let ok = SpectrumFrame::quantize(1, 2, 3, -1.0, 0.0, &[0.0; 4]).encode();
        assert_eq!(SpectrumFrame::decode(&ok[..10]), Err(FrameError::Length));
        assert_eq!(
            SpectrumFrame::decode(&ok[..ok.len() - 1]),
            Err(FrameError::Length)
        );
        let mut extra = ok.clone();
        extra.push(0);
        assert_eq!(SpectrumFrame::decode(&extra), Err(FrameError::Length));
        let mut v = ok.clone();
        v[0] = 2;
        assert_eq!(SpectrumFrame::decode(&v), Err(FrameError::Version(2)));
        let mut v = ok;
        v[1] = 9;
        assert_eq!(SpectrumFrame::decode(&v), Err(FrameError::Format(9)));
    }
}
