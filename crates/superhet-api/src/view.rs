//! Per-client spectrum view: control message parsing and crop/decimate.
//!
//! Every output frame is a regular [`SpectrumFrame`] (wire format v1), so the frontend needs
//! exactly one decoder: the frame header describes the cropped span (`center_hz`,
//! `sample_rate` = spanned bandwidth) and the decimated `bin_count`.

use std::time::Duration;

use serde::Deserialize;
use superhet_bus::spectrum::SpectrumFrame;

/// Smallest accepted target bin count.
pub const MIN_BINS: usize = 16;
/// Largest accepted target bin count (client pixel width).
pub const MAX_BINS: usize = 4096;
/// Bin count used until the client sends a view.
pub const DEFAULT_BINS: usize = 1024;
/// Lowest accepted frame rate limit.
pub const MIN_FPS: f64 = 1.0;
/// Highest accepted frame rate limit (also the default).
pub const MAX_FPS: f64 = 30.0;
/// Frequencies above this are rejected (1 THz, far beyond any supported receiver).
const MAX_HZ: f64 = 1e12;

/// Control message sent by the client as a JSON text frame.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    /// Select the visible span and resolution.
    View(ViewMessage),
}

/// Raw, unvalidated `view` message.
///
/// Either `start_hz` + `end_hz`, or `center_hz` + `span_hz`, or none of them (full frame).
#[derive(Debug, Default, Deserialize)]
pub struct ViewMessage {
    /// Protocol version; must be 1 if present.
    pub v: Option<u32>,
    /// Lower edge of the visible span in Hz.
    pub start_hz: Option<f64>,
    /// Upper edge of the visible span in Hz.
    pub end_hz: Option<f64>,
    /// Centre of the visible span in Hz.
    pub center_hz: Option<f64>,
    /// Width of the visible span in Hz.
    pub span_hz: Option<f64>,
    /// Target bin count (client pixel width); clamped to [`MIN_BINS`]..=[`MAX_BINS`].
    pub bins: Option<u32>,
    /// Frame rate limit; clamped to [`MIN_FPS`]..=[`MAX_FPS`].
    pub max_fps: Option<f64>,
}

/// Why a control message was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ViewError {
    /// Not valid JSON or not a known message.
    #[error("malformed message")]
    Malformed,
    /// Unsupported protocol version.
    #[error("unsupported protocol version")]
    Version,
    /// Span fields are inconsistent or out of range.
    #[error("invalid span")]
    Span,
    /// Frame rate is not a positive finite number.
    #[error("invalid max_fps")]
    Fps,
}

/// A validated client view.
#[derive(Debug, Clone, PartialEq)]
pub struct View {
    /// Requested span `(start_hz, end_hz)`; `None` means the full frame.
    pub span: Option<(f64, f64)>,
    /// Target number of output bins, within [`MIN_BINS`]..=[`MAX_BINS`].
    pub bins: usize,
    /// Minimum time between two frames sent to the client.
    pub min_interval: Duration,
}

impl Default for View {
    fn default() -> Self {
        Self {
            span: None,
            bins: DEFAULT_BINS,
            min_interval: Duration::from_secs_f64(1.0 / MAX_FPS),
        }
    }
}

fn valid_hz(v: f64) -> bool {
    v.is_finite() && (0.0..=MAX_HZ).contains(&v)
}

impl ViewMessage {
    /// Validate and clamp into a [`View`].
    ///
    /// # Errors
    /// Rejects unsupported versions, mixed/incomplete/out-of-range span fields and bad rates.
    pub fn validate(&self) -> Result<View, ViewError> {
        if self.v.is_some_and(|v| v != 1) {
            return Err(ViewError::Version);
        }
        let span = match (self.start_hz, self.end_hz, self.center_hz, self.span_hz) {
            (None, None, None, None) => None,
            (Some(start), Some(end), None, None) => Some((start, end)),
            (None, None, Some(center), Some(span)) if span.is_finite() => {
                Some((center - span / 2.0, center + span / 2.0))
            }
            _ => return Err(ViewError::Span),
        };
        // A centre/span pair may reach below 0 Hz; the lower edge is clamped to 0.
        let span = span.map(|(start, end)| (start.max(0.0), end));
        if let Some((start, end)) = span
            && (!valid_hz(start) || !valid_hz(end) || end - start < 1.0)
        {
            return Err(ViewError::Span);
        }
        let bins = self
            .bins
            .map_or(DEFAULT_BINS, |b| (b as usize).clamp(MIN_BINS, MAX_BINS));
        let fps = match self.max_fps {
            None => MAX_FPS,
            Some(f) if f.is_finite() && f > 0.0 => f.clamp(MIN_FPS, MAX_FPS),
            Some(_) => return Err(ViewError::Fps),
        };
        Ok(View {
            span,
            bins,
            min_interval: Duration::from_secs_f64(1.0 / fps),
        })
    }
}

/// Parse a JSON text frame from the client into a validated view.
///
/// # Errors
/// See [`ViewError`].
pub fn parse_control(text: &str) -> Result<View, ViewError> {
    match serde_json::from_str::<ControlMessage>(text).map_err(|_| ViewError::Malformed)? {
        ControlMessage::View(msg) => msg.validate(),
    }
}

/// Crop `frame` to the view's span and max-decimate to at most `view.bins` bins.
///
/// The requested span is clamped to the frame and snapped outwards to source bin edges, so the
/// returned header describes exactly what the bins cover. Frames are never upsampled: if the
/// cropped region has fewer bins than requested, they are passed through unchanged.
/// Decimation takes the maximum of each source group, so narrow peaks survive.
///
/// Returns `None` if the frame is empty/degenerate or the span does not overlap it.
#[must_use]
pub fn crop_decimate(frame: &SpectrumFrame, view: &View) -> Option<SpectrumFrame> {
    let n = frame.bins.len();
    let rate = u128::from(frame.sample_rate);
    if n == 0 || rate == 0 {
        return None;
    }
    let n128 = n as u128;
    // Frame lower edge in Hz (may be negative for a zero-IF tuner near DC).
    let f0 = i128::from(frame.center_hz) - i128::from(frame.sample_rate / 2);

    let (lo, hi) = match view.span {
        None => (0, n),
        Some((start, end)) => {
            // Work in bin units relative to f0; the span has been range-checked, so the casts
            // below only see values within +-1e12.
            let to_bin = |hz: f64| (hz - f0 as f64) * n as f64 / rate as f64;
            let lo_f = to_bin(start).floor().max(0.0);
            let hi_f = to_bin(end).ceil().min(n as f64);
            if hi_f <= lo_f {
                return None;
            }
            (lo_f as usize, hi_f as usize)
        }
    };
    let src = frame.bins.get(lo..hi)?;
    let len = src.len();
    let out_bins = if len <= view.bins {
        src.to_vec()
    } else {
        let m = view.bins;
        (0..m)
            .map(|j| {
                let a = j * len / m;
                let b = ((j + 1) * len / m).max(a + 1);
                src[a..b].iter().copied().max().unwrap_or(0)
            })
            .collect()
    };

    // Exact edges of the covered region, rounded to whole Hz.
    let lo128 = lo as u128;
    let len128 = len as u128;
    let start = f0 + i128::try_from(lo128 * rate / n128).ok()?;
    let width = (len128 * rate / n128).max(1);
    let center = (start + i128::try_from(width / 2).ok()?).max(0);
    Some(SpectrumFrame {
        center_hz: u64::try_from(center).ok()?,
        sample_rate: u32::try_from(width).ok()?,
        timestamp_us: frame.timestamp_us,
        min_db: frame.min_db,
        max_db: frame.max_db,
        bins: out_bins,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1000 bins over 1 MHz around 100 MHz: bin `i` covers `99.5 MHz + i * 1 kHz`.
    fn frame(bins: Vec<u8>) -> SpectrumFrame {
        SpectrumFrame {
            center_hz: 100_000_000,
            sample_rate: 1_000_000,
            timestamp_us: 42,
            min_db: -100.0,
            max_db: 0.0,
            bins,
        }
    }

    fn view(span: Option<(f64, f64)>, bins: usize) -> View {
        View {
            span,
            bins,
            ..View::default()
        }
    }

    #[test]
    fn full_frame_passes_through_when_small_enough() {
        let f = frame((0..1000).map(|i| (i % 256) as u8).collect());
        let out = crop_decimate(&f, &view(None, 1000)).expect("frame");
        assert_eq!(out, f);
    }

    #[test]
    fn decimation_preserves_peaks() {
        let mut bins = vec![10u8; 1000];
        bins[501] = 200;
        bins[999] = 99;
        let out = crop_decimate(&frame(bins), &view(None, 100)).expect("frame");
        assert_eq!(out.bins.len(), 100);
        assert_eq!(out.bins[50], 200);
        assert_eq!(out.bins[99], 99);
        assert_eq!(out.bins[0], 10);
        assert_eq!(out.center_hz, 100_000_000);
        assert_eq!(out.sample_rate, 1_000_000);
        assert_eq!(out.timestamp_us, 42);
    }

    #[test]
    fn decimation_with_uneven_ratio_covers_every_source_bin() {
        // 1000 -> 333: every source bin must land in exactly one group.
        for peak in [0usize, 1, 332, 500, 998, 999] {
            let mut bins = vec![1u8; 1000];
            bins[peak] = 250;
            let out = crop_decimate(&frame(bins), &view(None, 333)).expect("frame");
            assert_eq!(out.bins.len(), 333);
            assert_eq!(out.bins.iter().filter(|b| **b == 250).count(), 1, "{peak}");
        }
    }

    #[test]
    fn crop_selects_the_requested_span() {
        let bins: Vec<u8> = (0..1000).map(|i| (i / 4) as u8).collect();
        // 99.6 .. 99.7 MHz = source bins 100..200.
        let out = crop_decimate(
            &frame(bins),
            &view(Some((99_600_000.0, 99_700_000.0)), 4096),
        )
        .expect("frame");
        assert_eq!(out.bins.len(), 100);
        assert_eq!(out.bins[0], 25);
        assert_eq!(out.bins[99], 49);
        assert_eq!(out.sample_rate, 100_000);
        assert_eq!(out.center_hz, 99_650_000);
    }

    #[test]
    fn crop_snaps_outwards_to_bin_edges() {
        let out = crop_decimate(
            &frame(vec![5; 1000]),
            &view(Some((99_600_400.0, 99_600_600.0)), 4096),
        )
        .expect("frame");
        // 99.6004..99.6006 MHz sits inside source bin 100 (99.600..99.601 MHz).
        assert_eq!(out.bins.len(), 1);
        assert_eq!(out.sample_rate, 1_000);
        assert_eq!(out.center_hz, 99_600_500);
    }

    #[test]
    fn span_is_clamped_to_the_frame() {
        let f = frame(vec![7; 1000]);
        let out = crop_decimate(&f, &view(Some((0.0, 1e9)), 1000)).expect("frame");
        assert_eq!(out, f);
        let out = crop_decimate(&f, &view(Some((99_900_000.0, 1e9)), 4096)).expect("frame");
        assert_eq!(out.bins.len(), 600);
        assert_eq!(out.center_hz, 100_200_000);
    }

    #[test]
    fn span_outside_the_frame_yields_nothing() {
        let f = frame(vec![7; 1000]);
        assert!(crop_decimate(&f, &view(Some((1.0e6, 2.0e6)), 100)).is_none());
        assert!(crop_decimate(&f, &view(Some((101.0e6, 102.0e6)), 100)).is_none());
    }

    #[test]
    fn degenerate_frames_yield_nothing() {
        let mut f = frame(vec![]);
        assert!(crop_decimate(&f, &View::default()).is_none());
        f.bins = vec![1; 10];
        f.sample_rate = 0;
        assert!(crop_decimate(&f, &View::default()).is_none());
    }

    #[test]
    fn frame_near_dc_does_not_underflow() {
        let f = SpectrumFrame {
            center_hz: 100_000,
            sample_rate: 1_000_000,
            timestamp_us: 0,
            min_db: -100.0,
            max_db: 0.0,
            bins: vec![3; 1000],
        };
        let out = crop_decimate(&f, &view(Some((0.0, 200_000.0)), 4096)).expect("frame");
        assert!(out.sample_rate > 0 && out.bins.len() <= 1000);
    }

    #[test]
    fn parses_start_end_view() {
        let v = parse_control(
            r#"{"type":"view","v":1,"start_hz":1e6,"end_hz":2000000,"bins":800,"max_fps":10}"#,
        )
        .expect("valid");
        assert_eq!(v.span, Some((1e6, 2e6)));
        assert_eq!(v.bins, 800);
        assert_eq!(v.min_interval, Duration::from_millis(100));
    }

    #[test]
    fn parses_center_span_view() {
        let v = parse_control(r#"{"type":"view","center_hz":100000000,"span_hz":200000}"#)
            .expect("valid");
        assert_eq!(v.span, Some((99_900_000.0, 100_100_000.0)));
        assert_eq!(v.bins, DEFAULT_BINS);
    }

    #[test]
    fn empty_view_selects_full_frame() {
        let v = parse_control(r#"{"type":"view"}"#).expect("valid");
        assert_eq!(v, View::default());
    }

    #[test]
    fn clamps_bins_and_fps() {
        let v = parse_control(r#"{"type":"view","bins":1,"max_fps":1000}"#).expect("valid");
        assert_eq!(v.bins, MIN_BINS);
        assert_eq!(v.min_interval, Duration::from_secs_f64(1.0 / MAX_FPS));
        let v = parse_control(r#"{"type":"view","bins":4000000000,"max_fps":0.01}"#).expect("ok");
        assert_eq!(v.bins, MAX_BINS);
        assert_eq!(v.min_interval, Duration::from_secs_f64(1.0 / MIN_FPS));
    }

    #[test]
    fn negative_lower_edge_is_clamped_to_zero() {
        let v = parse_control(r#"{"type":"view","center_hz":100,"span_hz":1000}"#).expect("ok");
        assert_eq!(v.span, Some((0.0, 600.0)));
    }

    #[test]
    fn rejects_bad_input() {
        for (text, err) in [
            ("not json", ViewError::Malformed),
            (r#"{"type":"nope"}"#, ViewError::Malformed),
            (r#"{"type":"view","bins":-5}"#, ViewError::Malformed),
            (r#"{"type":"view","bins":"x"}"#, ViewError::Malformed),
            (r#"{"type":"view","v":2}"#, ViewError::Version),
            (r#"{"type":"view","start_hz":1e6}"#, ViewError::Span),
            (
                r#"{"type":"view","start_hz":1e6,"end_hz":2e6,"span_hz":5}"#,
                ViewError::Span,
            ),
            (
                r#"{"type":"view","start_hz":2e6,"end_hz":1e6}"#,
                ViewError::Span,
            ),
            (
                r#"{"type":"view","start_hz":-5,"end_hz":-1}"#,
                ViewError::Span,
            ),
            (
                r#"{"type":"view","start_hz":1e6,"end_hz":1e15}"#,
                ViewError::Span,
            ),
            (
                r#"{"type":"view","center_hz":1e6,"span_hz":0}"#,
                ViewError::Span,
            ),
            (r#"{"type":"view","max_fps":0}"#, ViewError::Fps),
            (r#"{"type":"view","max_fps":-1}"#, ViewError::Fps),
        ] {
            assert_eq!(parse_control(text), Err(err), "{text}");
        }
    }
}
