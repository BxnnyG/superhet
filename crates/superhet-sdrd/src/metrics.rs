//! Prometheus metrics of the daemon (`GET /metrics` on the internal port 8074).
//!
//! One process owns exactly one device, so the counters are process globals: the hot paths
//! (device callback consumer, VFO thread, publishers) bump a relaxed atomic and never take
//! a lock or allocate. All metrics are CPU-independent counters and gauges; see `docs/perf.md`
//! for what they mean and how to read them.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use superhet_bus::metrics::{Exposition, Kind};

use crate::overload::OverloadStatus;
use crate::vfo::{Status, Tuning, Vfo};

/// Lossy outputs that count dropped frames, in label order.
const STREAMS: [&str; 5] = ["spectrum", "zoom", "audio", "rds", "tones"];

/// Process-wide counters.
#[derive(Debug)]
pub struct Metrics {
    /// IQ blocks the DSP thread processed.
    blocks: AtomicU64,
    /// IQ samples the DSP thread processed.
    samples: AtomicU64,
    /// Blocks the device backend dropped because the DSP thread lagged.
    device_dropped_blocks: AtomicU64,
    /// Blocks the DSP thread dropped on the way to the VFO thread because that lagged.
    vfo_dropped_blocks: AtomicU64,
    /// Frames the publishers dropped (bus down or lagging), per entry of [`STREAMS`].
    frames_dropped: [AtomicU64; STREAMS.len()],
    /// RDS block error rate in percent per VFO id, for VFOs with a rate.
    rds_bler: Mutex<BTreeMap<u32, u8>>,
}

static METRICS: Metrics = Metrics {
    blocks: AtomicU64::new(0),
    samples: AtomicU64::new(0),
    device_dropped_blocks: AtomicU64::new(0),
    vfo_dropped_blocks: AtomicU64::new(0),
    frames_dropped: [const { AtomicU64::new(0) }; STREAMS.len()],
    rds_bler: Mutex::new(BTreeMap::new()),
};

/// The process-wide counters.
#[must_use]
pub fn global() -> &'static Metrics {
    &METRICS
}

impl Metrics {
    /// Accounts for one processed IQ block.
    pub fn block_processed(&self, samples: usize) {
        self.blocks.fetch_add(1, Ordering::Relaxed);
        self.samples.fetch_add(samples as u64, Ordering::Relaxed);
    }

    /// Records the device backend's running total of dropped blocks.
    pub fn set_device_dropped_blocks(&self, total: u64) {
        self.device_dropped_blocks.store(total, Ordering::Relaxed);
    }

    /// Total blocks dropped between device and DSP thread.
    #[must_use]
    pub fn device_dropped_blocks(&self) -> u64 {
        self.device_dropped_blocks.load(Ordering::Relaxed)
    }

    /// Counts a block dropped on the way to the VFO thread.
    pub fn vfo_block_dropped(&self) {
        self.vfo_dropped_blocks.fetch_add(1, Ordering::Relaxed);
    }

    /// Total blocks dropped on the way to the VFO thread.
    #[must_use]
    pub fn vfo_dropped_blocks(&self) -> u64 {
        self.vfo_dropped_blocks.load(Ordering::Relaxed)
    }

    /// Counts a dropped frame of output `stream` (one of `spectrum`, `zoom`, `audio`, `rds`,
    /// `tones`) and returns the new total of that output.
    pub fn frame_dropped(&self, stream: &str) -> u64 {
        STREAMS.iter().position(|s| *s == stream).map_or(0, |i| {
            self.frames_dropped[i].fetch_add(1, Ordering::Relaxed) + 1
        })
    }

    /// Sets (or with `None` removes) the RDS block error rate of a VFO.
    pub fn set_rds_bler(&self, vfo: u32, bler: Option<u8>) {
        let mut map = self
            .rds_bler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match bler {
            Some(bler) => {
                map.insert(vfo, bler);
            }
            None => {
                map.remove(&vfo);
            }
        }
    }

    /// Renders the exposition for the device `device`; `vfos` and `tuning` describe the
    /// current VFO set, `overload` the last ADC clipping interval.
    #[must_use]
    pub fn render(
        &self,
        device: &str,
        tuning: Option<Tuning>,
        vfos: &[Vfo],
        overload: &OverloadStatus,
    ) -> String {
        let dev = [("device", device)];
        let mut e = Exposition::new();
        e.single_u64(
            "superhet_sdrd_iq_blocks_total",
            "IQ blocks processed by the DSP thread.",
            Kind::Counter,
            self.blocks.load(Ordering::Relaxed),
        );
        e.single_u64(
            "superhet_sdrd_iq_samples_total",
            "IQ samples processed by the DSP thread.",
            Kind::Counter,
            self.samples.load(Ordering::Relaxed),
        );
        e.family(
            "superhet_sdrd_dropped_blocks_total",
            "IQ blocks dropped because a consumer lagged, by stage.",
            Kind::Counter,
        )
        .sample_u64(
            "superhet_sdrd_dropped_blocks_total",
            &[("stage", "device")],
            self.device_dropped_blocks(),
        )
        .sample_u64(
            "superhet_sdrd_dropped_blocks_total",
            &[("stage", "vfo")],
            self.vfo_dropped_blocks(),
        );
        e.family(
            "superhet_sdrd_dropped_frames_total",
            "Output frames dropped because the bus was down or lagging, by stream.",
            Kind::Counter,
        );
        for (i, stream) in STREAMS.iter().enumerate() {
            e.sample_u64(
                "superhet_sdrd_dropped_frames_total",
                &[("stream", stream)],
                self.frames_dropped[i].load(Ordering::Relaxed),
            );
        }
        let (clipped_percent, overloaded) = overload.get();
        e.family(
            "superhet_sdrd_clipped_percent",
            "Share of IQ components at the ADC rails in the last interval.",
            Kind::Gauge,
        )
        .sample_f64("superhet_sdrd_clipped_percent", &dev, clipped_percent);
        e.family(
            "superhet_sdrd_adc_overload",
            "1 while the ADC counts as overloaded (more than 1 percent clipped).",
            Kind::Gauge,
        )
        .sample_u64("superhet_sdrd_adc_overload", &dev, u64::from(overloaded));
        if let Some(tuning) = tuning {
            e.family(
                "superhet_sdrd_sample_rate_hertz",
                "Current device sample rate.",
                Kind::Gauge,
            )
            .sample_u64(
                "superhet_sdrd_sample_rate_hertz",
                &dev,
                u64::from(tuning.sample_rate),
            );
            let active = vfos
                .iter()
                .filter(|v| v.status(tuning) == Status::Active)
                .count() as u64;
            e.family(
                "superhet_sdrd_vfos",
                "VFOs by state (active = demodulating, paused = outside the window).",
                Kind::Gauge,
            )
            .sample_u64("superhet_sdrd_vfos", &[("state", "active")], active)
            .sample_u64(
                "superhet_sdrd_vfos",
                &[("state", "paused")],
                vfos.len() as u64 - active,
            );
        }
        e.family(
            "superhet_sdrd_rds_bler_percent",
            "RDS block error rate over the last seconds, per WFM VFO with RDS sync.",
            Kind::Gauge,
        );
        let blers = self
            .rds_bler
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        for vfo in vfos {
            if let Some(bler) = blers.get(&vfo.id) {
                e.sample_u64(
                    "superhet_sdrd_rds_bler_percent",
                    &[("vfo", &vfo.id.to_string())],
                    u64::from(*bler),
                );
            }
        }
        e.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfo::VfoMode;

    fn vfo(id: u32, freq_hz: u64) -> Vfo {
        Vfo {
            id,
            freq_hz,
            mode: VfoMode::Nfm,
            bandwidth_hz: None,
            squelch_dbfs: None,
            deemphasis: None,
            cw_pitch_hz: None,
            stereo: None,
            tones: None,
            tone_squelch_hz: None,
            owner: None,
        }
    }

    #[test]
    fn exposition_has_counters_gauges_and_vfo_states() {
        let metrics = Metrics {
            blocks: AtomicU64::new(7),
            samples: AtomicU64::new(7000),
            device_dropped_blocks: AtomicU64::new(3),
            vfo_dropped_blocks: AtomicU64::new(0),
            frames_dropped: [const { AtomicU64::new(0) }; STREAMS.len()],
            rds_bler: Mutex::new(BTreeMap::new()),
        };
        assert_eq!(metrics.frame_dropped("audio"), 1);
        assert_eq!(metrics.frame_dropped("audio"), 2);
        assert_eq!(metrics.frame_dropped("nonsense"), 0);
        metrics.set_rds_bler(1, Some(12));
        metrics.set_rds_bler(9, Some(50));
        let overload = OverloadStatus::default();
        overload.set(0.073, true);
        let tuning = Tuning {
            center_hz: 100_000_000,
            sample_rate: 2_048_000,
        };
        // 1 is inside the window, 2 is far outside and therefore paused.
        let vfos = [vfo(1, 100_000_000), vfo(2, 140_000_000)];
        let text = metrics.render("rtl0", Some(tuning), &vfos, &overload);
        for line in [
            "superhet_sdrd_iq_blocks_total 7",
            "superhet_sdrd_dropped_blocks_total{stage=\"device\"} 3",
            "superhet_sdrd_dropped_frames_total{stream=\"audio\"} 2",
            "superhet_sdrd_clipped_percent{device=\"rtl0\"} 7.3",
            "superhet_sdrd_adc_overload{device=\"rtl0\"} 1",
            "superhet_sdrd_vfos{state=\"active\"} 1",
            "superhet_sdrd_vfos{state=\"paused\"} 1",
            "superhet_sdrd_rds_bler_percent{vfo=\"1\"} 12",
        ] {
            assert!(text.lines().any(|l| l == line), "missing {line}\n{text}");
        }
        // A VFO that no longer exists does not leak its stale rate.
        assert!(!text.contains("vfo=\"9\""), "{text}");
    }
}
