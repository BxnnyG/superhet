//! Running the external tool and forwarding its output as events.

use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStdin, Command, ExitCode, Stdio};
use std::sync::mpsc;

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

/// Like [`run`], with several instances of the tool on the same input (each argument list starts
/// one). The adapter copies its `stdin` to all of them and merges what they print. The first
/// failing child ends the adapter with its exit code (the supervisor restarts the plugin).
pub fn run_many(
    program: &str,
    arg_sets: &[Vec<String>],
    mut map: impl FnMut(&str) -> Option<Value>,
) -> ExitCode {
    if let [only] = arg_sets {
        return run(program, only, map);
    }
    let mut children = Vec::new();
    for args in arg_sets {
        match Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
        {
            Ok(child) => children.push(child),
            Err(err) => {
                eprintln!("cannot start {program}: {err} (is the package installed?)");
                for mut started in children {
                    let _ = started.kill();
                }
                return ExitCode::from(127);
            }
        }
    }
    let mut pipes: Vec<ChildStdin> = children.iter_mut().filter_map(|c| c.stdin.take()).collect();
    let (tx, rx) = mpsc::channel::<Message>();
    for (index, mut child) in children.into_iter().enumerate() {
        let Some(stdout) = child.stdout.take() else {
            continue;
        };
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut raw = Vec::new();
            loop {
                raw.clear();
                match reader.read_until(b'\n', &mut raw) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let line = String::from_utf8_lossy(&raw).into_owned();
                        if tx.send(Message::Line(line)).is_err() {
                            return;
                        }
                    }
                }
            }
            let _ = tx.send(Message::Exit(index, child.wait().ok()));
        });
    }
    drop(tx);
    // Copy stdin to every child; the pipes close when the host closes our stdin (or on exit).
    std::thread::spawn(move || {
        let mut stdin = std::io::stdin().lock();
        let mut buf = vec![0u8; 16 * 1024];
        while !pipes.is_empty() {
            match std::io::Read::read(&mut stdin, &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => pipes.retain_mut(|pipe| pipe.write_all(&buf[..n]).is_ok()),
            }
        }
    });
    let mut out = std::io::stdout().lock();
    while let Ok(message) = rx.recv() {
        match message {
            Message::Line(line) => {
                if let Some(event) = map(&line)
                    && writeln!(out, "{event}").and_then(|()| out.flush()).is_err()
                {
                    return ExitCode::FAILURE;
                }
            }
            Message::Exit(index, status) => {
                if status.is_some_and(|s| s.success()) {
                    continue;
                }
                eprintln!("{program} #{index} ended with {status:?}");
                // One decoder is gone: end the adapter so the host restarts the plugin.
                return ExitCode::from(
                    status
                        .and_then(|s| s.code())
                        .and_then(|c| u8::try_from(c).ok())
                        .unwrap_or(1),
                );
            }
        }
    }
    ExitCode::SUCCESS
}

enum Message {
    Line(String),
    Exit(usize, Option<std::process::ExitStatus>),
}
