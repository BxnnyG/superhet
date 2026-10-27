//! The DAB / DAB+ plugin (`plugins/dab`, ADR-0016): a framed-transport adapter around
//! `welle-cli` (welle.io, GPL-2.0-or-later, a separate process).
//!
//! ```text
//!  host (cu8 IQ in `samples` frames) --> FakeRtl (rtl_tcp) --> welle-cli -w <port>
//!  host <-- events / status / media / audio <-- engine <-- GET /mux.json /slide/* /mp3/*
//!  host --> control {tune, select, scan} --> engine --> tune frame (retune the window)
//! ```
//!
//! The pure parts (channel table, `mux.json` parser, dwell decision, MP3 to PCM conversion, the
//! control vocabulary) are unit-tested; `tests/dab_plugin.rs` runs the whole adapter against a
//! fake host and a fake `welle-cli`.

pub mod audio;
pub mod channels;
pub mod dwell;
pub mod engine;
pub mod http;
pub mod link;
pub mod mux;
pub mod rtl_tcp;

use std::io::Read;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::time::Duration;

use serde_json::{Value, json};
use superhet_plugin_protocol::framed::{Frame, FrameKind, SamplesHeader};

use engine::{Cmd, Engine, Setup, parse_control};

/// Interval of the plugin's heartbeat frames (the host's watchdog allows 15 s).
const HEARTBEAT_EVERY: Duration = Duration::from_secs(3);
/// IQ chunks waiting for the stand-in rtl_tcp server (about 2.5 s); more are dropped.
const IQ_QUEUE: usize = 128;

/// Run the plugin: connect to the host, serve until it goes away. `program` is the decoder to
/// start (`welle-cli`).
#[must_use]
pub fn run(program: &str, config: &Value, socket: &str, freq_hz: Option<u64>) -> ExitCode {
    let (link, mut incoming, hello) = match link::connect(socket) {
        Ok(parts) => parts,
        Err(err) => {
            eprintln!("cannot reach the host at {socket}: {err}");
            return ExitCode::from(2);
        }
    };
    let link = Arc::new(link);
    let fake = match rtl_tcp::FakeRtl::start() {
        Ok(fake) => Arc::new(fake),
        Err(err) => {
            let _ = link.log(&format!("cannot open the local IQ server: {err}"));
            return ExitCode::FAILURE;
        }
    };
    let muted = Arc::new(AtomicBool::new(false));

    let channel = freq_hz.or(hello.freq_hz).and_then(channels::name_of);
    let service = config["service"]
        .as_str()
        .and_then(|s| u32::from_str_radix(s, 16).ok());
    let (commands, command_rx) = mpsc::channel::<Cmd>();
    let engine = Engine::new(Setup {
        link: Arc::clone(&link),
        fake: Arc::clone(&fake),
        muted: Arc::clone(&muted),
        channel,
        current_freq_hz: freq_hz.or(hello.freq_hz),
        service,
        timing: timing_from(config),
        program: program.to_owned(),
    });
    let engine_thread = {
        let link = Arc::clone(&link);
        std::thread::spawn(move || {
            engine.run(&command_rx);
            // Wakes the reader below when the engine ended on its own (stdin closed).
            link.close();
        })
    };

    // Heartbeats: the host kills a framed plugin that stays silent.
    {
        let link = Arc::clone(&link);
        std::thread::spawn(move || {
            while link.heartbeat().is_ok() {
                std::thread::sleep(HEARTBEAT_EVERY);
            }
        });
    }
    // The host closes stdin to ask the plugin to end.
    {
        let commands = commands.clone();
        std::thread::spawn(move || {
            let mut sink = [0u8; 64];
            while std::io::stdin().read(&mut sink).is_ok_and(|n| n > 0) {}
            let _ = commands.send(Cmd::Shutdown);
        });
    }
    // IQ goes through its own thread so a slow welle-cli never blocks the control path.
    let (iq, iq_rx) = mpsc::sync_channel::<Vec<u8>>(IQ_QUEUE);
    {
        let (fake, muted) = (Arc::clone(&fake), Arc::clone(&muted));
        std::thread::spawn(move || {
            for chunk in iq_rx {
                if !muted.load(Ordering::Relaxed) {
                    fake.push(&chunk);
                }
            }
        });
    }

    loop {
        match incoming.read_frame() {
            Ok(Some(frame)) => handle(frame, &iq, &commands, &link),
            Ok(None) => break,
            Err(err) => {
                eprintln!("host connection: {err}");
                break;
            }
        }
    }
    let _ = commands.send(Cmd::Shutdown);
    let _ = engine_thread.join();
    ExitCode::SUCCESS
}

/// Dwell timing, overridable with `{"timing": {"no_signal_ms", "stable_ms", "max_ms"}}` in the
/// instance configuration (the tests use short values; operators rarely need to).
fn timing_from(config: &Value) -> dwell::Timing {
    let mut timing = dwell::Timing::default();
    let ms = |key: &str| config["timing"][key].as_u64().map(Duration::from_millis);
    if let Some(value) = ms("no_signal_ms") {
        timing.no_signal = value;
    }
    if let Some(value) = ms("stable_ms") {
        timing.stable = value;
    }
    if let Some(value) = ms("max_ms") {
        timing.max = value;
    }
    timing
}

fn handle(frame: Frame, iq: &SyncSender<Vec<u8>>, commands: &mpsc::Sender<Cmd>, link: &link::Link) {
    match frame.kind {
        FrameKind::Samples => {
            if let Ok((_, samples)) = SamplesHeader::decode(&frame.payload) {
                // Dropping on a full queue is the contract: a slow plugin loses samples.
                let _ = iq.try_send(samples.to_vec());
            }
        }
        FrameKind::Control => match serde_json::from_slice::<Value>(&frame.payload)
            .map_err(|err| err.to_string())
            .and_then(|message| parse_control(&message))
        {
            Ok(cmd) => {
                let _ = commands.send(cmd);
            }
            Err(message) => {
                let _ = link.event(
                    "dab.error",
                    &json!({"summary": format!("control message refused: {message}"), "message": message}),
                );
            }
        },
        _ => {}
    }
}
