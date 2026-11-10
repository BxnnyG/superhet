//! `ctl.scanner.*` request parsing and validation.

use std::collections::BTreeSet;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};

use super::machine::{Params, Resume};
use super::plan::{Channel, mode_default_bandwidth};
use crate::control::{CtlError, VERSION};
use crate::controller::default_mode;
use crate::vfo::VfoMode;

/// Most channels in a list scan.
pub const MAX_CHANNELS: usize = 2000;
/// Most channels a range scan may generate.
pub const MAX_RANGE_CHANNELS: usize = 20_000;
/// Most locked-out frequencies.
pub const MAX_LOCKOUTS: usize = 2000;

const FREQ_RANGE: std::ops::RangeInclusive<u64> = 1..=1_000_000_000_000;
const BANDWIDTH_RANGE: std::ops::RangeInclusive<u32> = 100..=1_000_000;
const THRESHOLD_DB_RANGE: std::ops::RangeInclusive<f32> = 1.0..=60.0;
const THRESHOLD_DBFS_RANGE: std::ops::RangeInclusive<f32> = -140.0..=0.0;
const DWELL_MS: std::ops::RangeInclusive<u32> = 20..=5_000;
const HANG_MS: std::ops::RangeInclusive<u32> = 0..=60_000;
const TIMEOUT_MS: std::ops::RangeInclusive<u32> = 500..=600_000;
const PRIORITY_MS: std::ops::RangeInclusive<u32> = 500..=600_000;
const LABEL_CHARS: usize = 64;

/// A validated `ctl.scanner.start`.
pub struct Start {
    /// The VFO to move; `None` = the lowest id.
    pub vfo: Option<u32>,
    /// What to watch.
    pub channels: Vec<Channel>,
    /// Thresholds and timing.
    pub params: Params,
    /// Frequencies to leave out.
    pub lockouts: Vec<u64>,
    /// Source and parameters as shown in the state document.
    pub echo: Value,
}

/// A scanner command.
pub enum Command {
    /// Start (or restart with new settings).
    Start(Box<Start>),
    /// Stop and give the tuning back.
    Stop,
    /// Freeze.
    Pause,
    /// Continue, or let go of a held channel.
    Resume,
    /// Leave the held channel / window.
    Skip,
    /// Lock out the frequency (`None`: the held channel).
    Lockout(Option<u64>),
    /// Remove a lockout (`None`: all).
    Unlock(Option<u64>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChannelWire {
    freq_hz: u64,
    mode: Option<VfoMode>,
    bandwidth_hz: Option<u32>,
    label: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum SourceWire {
    Range {
        start_hz: u64,
        stop_hz: u64,
        step_hz: u64,
        mode: Option<VfoMode>,
        bandwidth_hz: Option<u32>,
    },
    List {
        name: Option<String>,
        channels: Vec<ChannelWire>,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PriorityWire {
    freq_hz: u64,
    mode: Option<VfoMode>,
    bandwidth_hz: Option<u32>,
    label: Option<String>,
    interval_ms: Option<u32>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum ResumeWire {
    Hang,
    Timeout,
    Hold,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartWire {
    v: u32,
    vfo: Option<u32>,
    source: SourceWire,
    threshold_db: Option<f32>,
    threshold_dbfs: Option<f32>,
    dwell_ms: Option<u32>,
    hang_ms: Option<u32>,
    resume: Option<ResumeWire>,
    timeout_ms: Option<u32>,
    priority: Option<PriorityWire>,
    lockout_hz: Option<Vec<u64>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FreqWire {
    v: u32,
    freq_hz: Option<u64>,
    #[serde(default)]
    all: bool,
}

fn check<T: PartialOrd + std::fmt::Display>(
    name: &str,
    value: T,
    range: &std::ops::RangeInclusive<T>,
) -> Result<T, CtlError> {
    if range.contains(&value) {
        Ok(value)
    } else {
        Err(CtlError::invalid(format!(
            "{name} {value} outside {}..={}",
            range.start(),
            range.end()
        )))
    }
}

fn label(text: Option<String>) -> Result<Option<String>, CtlError> {
    match text {
        None => Ok(None),
        Some(t) => {
            let t = t.trim().to_owned();
            if t.is_empty() {
                Ok(None)
            } else if t.chars().count() > LABEL_CHARS || t.chars().any(char::is_control) {
                Err(CtlError::invalid(format!(
                    "label must be at most {LABEL_CHARS} printable characters"
                )))
            } else {
                Ok(Some(t))
            }
        }
    }
}

fn bandwidth(bw: Option<u32>) -> Result<Option<u32>, CtlError> {
    bw.map(|b| check("bandwidth_hz", b, &BANDWIDTH_RANGE))
        .transpose()
}

fn channel_from(wire: ChannelWire) -> Result<Channel, CtlError> {
    let freq_hz = check("freq_hz", wire.freq_hz, &FREQ_RANGE)?;
    Ok(Channel {
        freq_hz,
        mode: wire.mode.unwrap_or_else(|| default_mode(freq_hz)),
        bandwidth_hz: bandwidth(wire.bandwidth_hz)?,
        detect_hz: None,
        label: label(wire.label)?,
        priority: false,
    })
}

/// Parse a command; `name` is the part after `ctl.scanner.`.
///
/// # Errors
/// `invalid` for an unknown command or a malformed / out-of-range body.
pub fn parse_command(name: &str, payload: &[u8]) -> Result<Command, CtlError> {
    match name {
        "start" => parse_start(payload).map(|s| Command::Start(Box::new(s))),
        "stop" | "pause" | "resume" | "skip" => {
            let wire: FreqWire = parse(payload)?;
            version(wire.v)?;
            if wire.freq_hz.is_some() || wire.all {
                return Err(CtlError::invalid(format!("{name} takes no parameters")));
            }
            Ok(match name {
                "stop" => Command::Stop,
                "pause" => Command::Pause,
                "resume" => Command::Resume,
                _ => Command::Skip,
            })
        }
        "lockout" => {
            let wire: FreqWire = parse(payload)?;
            version(wire.v)?;
            if wire.all {
                return Err(CtlError::invalid("lockout takes freq_hz only"));
            }
            let freq = wire
                .freq_hz
                .map(|f| check("freq_hz", f, &FREQ_RANGE))
                .transpose()?;
            Ok(Command::Lockout(freq))
        }
        "unlock" => {
            let wire: FreqWire = parse(payload)?;
            version(wire.v)?;
            match (wire.freq_hz, wire.all) {
                (Some(_), true) | (None, false) => {
                    Err(CtlError::invalid("unlock takes freq_hz or all:true"))
                }
                (Some(f), false) => Ok(Command::Unlock(Some(check("freq_hz", f, &FREQ_RANGE)?))),
                (None, true) => Ok(Command::Unlock(None)),
            }
        }
        other => Err(CtlError::invalid(format!(
            "unknown scanner command {other:?}"
        ))),
    }
}

fn version(v: u32) -> Result<(), CtlError> {
    if v == VERSION {
        Ok(())
    } else {
        Err(CtlError::invalid("unsupported version"))
    }
}

fn parse<T: for<'de> Deserialize<'de>>(payload: &[u8]) -> Result<T, CtlError> {
    serde_json::from_slice(payload)
        .map_err(|err| CtlError::invalid(format!("malformed scanner request: {err}")))
}

fn parse_start(payload: &[u8]) -> Result<Start, CtlError> {
    let wire: StartWire = parse(payload)?;
    if wire.v != VERSION {
        return Err(CtlError::invalid("unsupported version"));
    }
    let (mut channels, source) = match wire.source {
        SourceWire::Range {
            start_hz,
            stop_hz,
            step_hz,
            mode,
            bandwidth_hz,
        } => {
            check("start_hz", start_hz, &FREQ_RANGE)?;
            check("stop_hz", stop_hz, &FREQ_RANGE)?;
            check("step_hz", step_hz, &(100..=100_000_000))?;
            if stop_hz < start_hz {
                return Err(CtlError::invalid("stop_hz is below start_hz"));
            }
            let count = (stop_hz - start_hz) / step_hz + 1;
            if count > MAX_RANGE_CHANNELS as u64 {
                return Err(CtlError::invalid(format!(
                    "range has {count} channels; at most {MAX_RANGE_CHANNELS} (use a larger step)"
                )));
            }
            let bandwidth_hz = bandwidth(bandwidth_hz)?;
            let channels = (0..count)
                .map(|i| {
                    let freq_hz = start_hz + i * step_hz;
                    let mode = mode.unwrap_or_else(|| default_mode(freq_hz));
                    let vfo_bw = bandwidth_hz.unwrap_or_else(|| mode_default_bandwidth(mode));
                    Channel {
                        freq_hz,
                        mode,
                        bandwidth_hz,
                        detect_hz: Some(vfo_bw.min(u32::try_from(step_hz).unwrap_or(u32::MAX))),
                        label: None,
                        priority: false,
                    }
                })
                .collect::<Vec<_>>();
            (
                channels,
                json!({"kind": "range", "start_hz": start_hz, "stop_hz": stop_hz,
                       "step_hz": step_hz, "count": count}),
            )
        }
        SourceWire::List { name, channels } => {
            if channels.is_empty() || channels.len() > MAX_CHANNELS {
                return Err(CtlError::invalid(format!(
                    "a channel list needs 1..={MAX_CHANNELS} channels"
                )));
            }
            let name = label(name)?;
            let mut seen = BTreeSet::new();
            let mut list = Vec::with_capacity(channels.len());
            for wire in channels {
                let channel = channel_from(wire)?;
                if seen.insert(channel.freq_hz) {
                    list.push(channel);
                }
            }
            let count = list.len();
            (list, json!({"kind": "list", "name": name, "count": count}))
        }
    };

    let mut priority_interval = 5_000;
    if let Some(p) = wire.priority {
        let channel = channel_from(ChannelWire {
            freq_hz: p.freq_hz,
            mode: p.mode,
            bandwidth_hz: p.bandwidth_hz,
            label: p.label,
        })?;
        if let Some(ms) = p.interval_ms {
            priority_interval = check("priority interval_ms", ms, &PRIORITY_MS)?;
        }
        match channels.iter_mut().find(|c| c.freq_hz == channel.freq_hz) {
            Some(existing) => existing.priority = true,
            None => channels.push(Channel {
                priority: true,
                ..channel
            }),
        }
    }

    let threshold_db = check(
        "threshold_db",
        wire.threshold_db.unwrap_or(10.0),
        &THRESHOLD_DB_RANGE,
    )?;
    let threshold_dbfs = wire
        .threshold_dbfs
        .map(|v| check("threshold_dbfs", v, &THRESHOLD_DBFS_RANGE))
        .transpose()?;
    let dwell_ms = check("dwell_ms", wire.dwell_ms.unwrap_or(150), &DWELL_MS)?;
    let hang_ms = check("hang_ms", wire.hang_ms.unwrap_or(2_000), &HANG_MS)?;
    let timeout_ms = check("timeout_ms", wire.timeout_ms.unwrap_or(10_000), &TIMEOUT_MS)?;
    let resume = match wire.resume {
        None | Some(ResumeWire::Hang) => Resume::Hang,
        Some(ResumeWire::Timeout) => Resume::Timeout,
        Some(ResumeWire::Hold) => Resume::Hold,
    };
    let lockouts = wire.lockout_hz.unwrap_or_default();
    if lockouts.len() > MAX_LOCKOUTS {
        return Err(CtlError::invalid(format!(
            "at most {MAX_LOCKOUTS} locked-out frequencies"
        )));
    }
    for f in &lockouts {
        check("lockout_hz", *f, &FREQ_RANGE)?;
    }
    let mut echo = source;
    echo["threshold_db"] = json!(threshold_db);
    echo["threshold_dbfs"] = json!(threshold_dbfs);
    echo["dwell_ms"] = json!(dwell_ms);
    echo["hang_ms"] = json!(hang_ms);
    echo["timeout_ms"] = json!(timeout_ms);
    echo["resume"] = json!(match resume {
        Resume::Hang => "hang",
        Resume::Timeout => "timeout",
        Resume::Hold => "hold",
    });
    echo["priority_hz"] = json!(channels.iter().find(|c| c.priority).map(|c| c.freq_hz));
    echo["priority_interval_ms"] = json!(priority_interval);
    Ok(Start {
        vfo: wire.vfo,
        channels,
        params: Params {
            threshold_db,
            threshold_dbfs,
            dwell: Duration::from_millis(u64::from(dwell_ms)),
            hang: Duration::from_millis(u64::from(hang_ms)),
            resume,
            timeout: Duration::from_millis(u64::from(timeout_ms)),
            priority_interval: Duration::from_millis(u64::from(priority_interval)),
        },
        lockouts,
        echo,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(body: &str) -> Result<Start, CtlError> {
        parse_start(body.as_bytes())
    }

    #[test]
    fn a_range_becomes_channels_with_defaults() {
        let s = start(
            r#"{"v":1,"source":{"kind":"range","start_hz":145000000,"stop_hz":145100000,"step_hz":12500}}"#,
        )
        .expect("valid");
        assert_eq!(s.channels.len(), 9);
        assert_eq!(s.channels[1].freq_hz, 145_012_500);
        assert_eq!(s.channels[0].mode, VfoMode::Nfm);
        // Measured over the step, not wider.
        assert_eq!(s.channels[0].detect_hz, Some(12_500));
        assert_eq!(s.params.dwell, Duration::from_millis(150));
        assert_eq!(s.echo["kind"], "range");
        assert_eq!(s.echo["count"], 9);
    }

    #[test]
    fn a_list_takes_modes_labels_and_a_priority() {
        let s = start(
            r#"{"v":1,"vfo":2,"source":{"kind":"list","name":"PMR","channels":[
                {"freq_hz":446006250,"label":"PMR 1"},
                {"freq_hz":446006250},
                {"freq_hz":118000000,"mode":"am","bandwidth_hz":8000}]},
               "priority":{"freq_hz":446018750,"interval_ms":2000},
               "resume":"timeout","timeout_ms":3000,"lockout_hz":[446093750]}"#,
        )
        .expect("valid");
        assert_eq!(s.vfo, Some(2));
        // The duplicate is dropped, the priority channel is added.
        assert_eq!(s.channels.len(), 3);
        assert_eq!(s.channels[0].label.as_deref(), Some("PMR 1"));
        assert_eq!(s.channels[1].mode, VfoMode::Am);
        assert!(s.channels[2].priority);
        assert_eq!(s.params.priority_interval, Duration::from_millis(2000));
        assert_eq!(s.params.resume, Resume::Timeout);
        assert_eq!(s.lockouts, vec![446_093_750]);
    }

    #[test]
    fn bad_requests_are_invalid() {
        for body in [
            r#"{"v":2,"source":{"kind":"list","channels":[{"freq_hz":1}]}}"#,
            r#"{"v":1,"source":{"kind":"list","channels":[]}}"#,
            r#"{"v":1,"source":{"kind":"range","start_hz":2,"stop_hz":1,"step_hz":1000}}"#,
            r#"{"v":1,"source":{"kind":"range","start_hz":1000,"stop_hz":900000000,"step_hz":100}}"#,
            r#"{"v":1,"source":{"kind":"range","start_hz":1000,"stop_hz":2000,"step_hz":5}}"#,
            r#"{"v":1,"threshold_db":0,"source":{"kind":"list","channels":[{"freq_hz":1000}]}}"#,
            r#"{"v":1,"dwell_ms":1,"source":{"kind":"list","channels":[{"freq_hz":1000}]}}"#,
            r#"{"v":1,"x":1,"source":{"kind":"list","channels":[{"freq_hz":1000}]}}"#,
            r#"{"v":1,"source":{"kind":"list","channels":[{"freq_hz":0}]}}"#,
            r#"{"v":1,"source":{"kind":"list","channels":[{"freq_hz":5,"label":"a\nb"}]}}"#,
        ] {
            assert!(start(body).is_err(), "{body}");
        }
    }

    #[test]
    fn simple_commands_validate_their_bodies() {
        assert!(matches!(
            parse_command("stop", br#"{"v":1}"#),
            Ok(Command::Stop)
        ));
        assert!(parse_command("stop", br#"{"v":1,"freq_hz":5}"#).is_err());
        assert!(matches!(
            parse_command("lockout", br#"{"v":1,"freq_hz":145500000}"#),
            Ok(Command::Lockout(Some(145_500_000)))
        ));
        assert!(matches!(
            parse_command("lockout", br#"{"v":1}"#),
            Ok(Command::Lockout(None))
        ));
        assert!(matches!(
            parse_command("unlock", br#"{"v":1,"all":true}"#),
            Ok(Command::Unlock(None))
        ));
        assert!(parse_command("unlock", br#"{"v":1}"#).is_err());
        assert!(parse_command("nope", br#"{"v":1}"#).is_err());
    }
}
