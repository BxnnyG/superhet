//! Hardware test: needs a plugged-in RTL-SDR and access rights (udev rules).
//! Run with `cargo test -p superhet-device --features rtlsdr -- --ignored --nocapture`.
#![cfg(feature = "rtlsdr")]

use std::sync::Mutex;
use std::time::{Duration, Instant};

use superhet_device::{Device, Gain, RtlSdrConfig, RtlSdrDevice, TuneRequest};

/// The dongle can only be opened once; serialise the tests instead of failing on a busy device.
static DONGLE: Mutex<()> = Mutex::new(());

#[test]
#[ignore = "requires an RTL-SDR dongle"]
fn streams_half_a_second_at_nominal_rate() {
    let _dongle = DONGLE.lock().unwrap_or_else(|e| e.into_inner());
    let cfg = RtlSdrConfig::parse("0?rate=2048000&freq=100000000&gain=auto").expect("config");
    let mut dev = RtlSdrDevice::open(cfg).expect("open device 0");
    println!("tuner: {}", dev.tuner());
    let rate = dev.sample_rate();
    assert!((2_000_000..=2_100_000).contains(&rate), "rate {rate}");

    let stream = dev.start(64).expect("start");
    // Discard the first block (start-up transient), then time the following 0.5 s.
    stream
        .blocks
        .recv_timeout(Duration::from_secs(5))
        .expect("first block");
    let begin = Instant::now();
    let mut samples = 0usize;
    while begin.elapsed() < Duration::from_millis(500) {
        let block = stream
            .blocks
            .recv_timeout(Duration::from_secs(2))
            .expect("block");
        assert_eq!(block.center_hz, 100_000_000);
        assert!(
            block
                .samples
                .iter()
                .all(|s| s.re.abs() <= 1.0 && s.im.abs() <= 1.0)
        );
        samples += block.samples.len();
    }
    let measured = samples as f64 / begin.elapsed().as_secs_f64();
    println!(
        "measured rate: {measured:.0} S/s, dropped blocks: {}",
        stream.dropped_blocks()
    );
    assert!(
        (measured - f64::from(rate)).abs() < f64::from(rate) * 0.15,
        "measured {measured}"
    );
    dev.stop();
    // The channel closes once the device has stopped.
    while stream.blocks.recv_timeout(Duration::from_secs(2)).is_ok() {}
}

#[test]
#[ignore = "requires an RTL-SDR dongle"]
fn retunes_while_streaming() {
    let _dongle = DONGLE.lock().unwrap_or_else(|e| e.into_inner());
    let cfg = RtlSdrConfig::parse("0?rate=2048000&freq=100000000&gain=auto").expect("config");
    let mut dev = RtlSdrDevice::open(cfg).expect("open device 0");
    let control = dev.control();
    let stream = dev.start(64).expect("start");
    stream
        .blocks
        .recv_timeout(Duration::from_secs(5))
        .expect("first block");

    let state = control
        .apply(&TuneRequest {
            center_hz: Some(433_920_000),
            sample_rate: Some(1_024_000),
            gain: Some(Gain::Db(30.0)),
            ppm: Some(1),
            bias_tee: Some(false),
        })
        .expect("retune");
    assert_eq!(state.center_hz, 433_920_000);
    assert_eq!(state.tuning_epoch, 1);
    println!("applied gain: {:?}", state.gain);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let block = stream
            .blocks
            .recv_timeout(Duration::from_secs(2))
            .expect("block");
        if block.tuning_epoch == 1 {
            assert_eq!(block.center_hz, 433_920_000);
            assert!((1_000_000..=1_050_000).contains(&block.sample_rate));
            break;
        }
        assert!(Instant::now() < deadline, "no block after the retune");
    }
}
