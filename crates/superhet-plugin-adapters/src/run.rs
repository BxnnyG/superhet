//! Running the external tool and forwarding its output as events.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, ExitCode, Stdio};

use serde_json::Value;

/// Start `program` with the plugin's stdin and feed every line it prints through `map`; the
/// events `map` returns are written to stdout, one JSON object per line.
///
/// The tool inherits `stdin` (so the host closing it ends the tool) and `stderr` (the host logs
/// it). The adapter exits with status 0 when the tool did, otherwise with the tool's code (127
/// when it could not be started, i.e. is not installed).
pub fn run(program: &str, args: &[String], mut map: impl FnMut(&str) -> Option<Value>) -> ExitCode {
    let mut child = match Command::new(program)
        .args(args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            eprintln!("cannot start {program}: {err} (is the package installed?)");
            return ExitCode::from(127);
        }
    };
    let Some(stdout) = child.stdout.take() else {
        eprintln!("no stdout from {program}");
        let _ = child.kill();
        return ExitCode::FAILURE;
    };
    let mut out = std::io::stdout().lock();
    let mut reader = BufReader::new(stdout);
    let mut raw = Vec::new();
    loop {
        raw.clear();
        match reader.read_until(b'\n', &mut raw) {
            Ok(0) => break,
            Ok(_) => {}
            Err(err) => {
                eprintln!("reading {program}: {err}");
                break;
            }
        }
        let line = String::from_utf8_lossy(&raw);
        if let Some(event) = map(&line) {
            // The host closed our stdout: nobody listens any more.
            if writeln!(out, "{event}").and_then(|()| out.flush()).is_err() {
                let _ = child.kill();
                let _ = child.wait();
                return ExitCode::FAILURE;
            }
        }
    }
    match child.wait() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => {
            eprintln!("{program} ended with {status}");
            ExitCode::from(
                status
                    .code()
                    .and_then(|c| u8::try_from(c).ok())
                    .unwrap_or(1),
            )
        }
        Err(err) => {
            eprintln!("waiting for {program}: {err}");
            ExitCode::FAILURE
        }
    }
}
