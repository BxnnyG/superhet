//! RDS from a real RTL-SDR recording (not part of the repository).

use num_complex::Complex32;

use crate::channel::{Channel, ChannelConfig, Deemphasis, Mode};

#[test]
#[ignore = "needs a real recording: SUPERHET_RDS_IQ=file.cu8 SUPERHET_RDS_RATE=2400000"]
fn real_recording_probe() {
    let Ok(path) = std::env::var("SUPERHET_RDS_IQ") else {
        return;
    };
    let rate: u32 = std::env::var("SUPERHET_RDS_RATE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(2_400_000);
    let raw = std::fs::read(path).expect("read recording");
    let iq: Vec<Complex32> = raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| {
            Complex32::new(
                (f32::from(c[0]) - 127.5) / 127.5,
                (f32::from(c[1]) - 127.5) / 127.5,
            )
        })
        .collect();
    for offset in [-810_000.0, 790_000.0, -10_000.0] {
        let mut ch = Channel::new(ChannelConfig {
            input_rate: rate,
            offset_hz: offset,
            mode: Mode::Wfm {
                deemphasis: Deemphasis::Us50,
            },
            bandwidth_hz: None,
            squelch_dbfs: None,
        })
        .expect("channel");
        let mut out = Vec::new();
        let mut synced = 0;
        let mut power = 0.0;
        for chunk in iq.chunks(rate as usize / 20) {
            power = ch.process(chunk, &mut out).power_dbfs;
            out.clear();
            synced += usize::from(ch.rds_synced());
        }
        eprintln!(
            "offset {offset}: power {power:.1} pilot {} synced {synced}/200 info {:?}",
            ch.stereo_locked(),
            ch.rds_info()
        );
    }
}
