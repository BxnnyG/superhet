//! The contract test suite for simple-transport plugins.
//!
//! [`run_simple`] launches a plugin the way the host does (working directory = plugin directory,
//! environment from [`crate::InstanceEnv`]), feeds it bytes on stdin, closes stdin and judges
//! what comes back: every stdout line must pass [`crate::EventValidator`] and the process must
//! exit successfully on its own after the input ended. [`check`] asserts that a [`Report`]
//! satisfies the contract. Plugin authors call both from a test; the crate's own tests run them
//! against `plugins/example-echo`.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::event::{Event, EventError, Line, MAX_EVENT_LINE};
use crate::package::Package;
use crate::{InstanceEnv, PROTOCOL_VERSION};

/// What a plugin did during one run.
#[derive(Debug)]
pub struct Report {
    /// Well-formed events (heartbeats excluded).
    pub events: Vec<Event>,
    /// Number of heartbeat lines.
    pub heartbeats: usize,
    /// Rejected stdout lines with the reason.
    pub violations: Vec<(String, EventError)>,
    /// Exit status, `None` when the run hit the timeout and the process was killed.
    pub exit: Option<ExitStatus>,
    /// Captured stderr (lossy UTF-8).
    pub stderr: String,
}

/// Run a simple-transport plugin once.
///
/// `input` is written to stdin, then stdin is closed. `timeout` bounds the whole run.
///
/// # Errors
/// I/O errors while spawning or talking to the process.
pub fn run_simple(
    package: &Package,
    input: &[u8],
    config: &Value,
    timeout: Duration,
) -> std::io::Result<Report> {
    run_simple_with_env(package, input, config, timeout, &[])
}

/// [`run_simple`] with extra environment variables for the child, e.g. a `PATH` that finds the
/// plugin's helper binaries (tests of plugins that call a program by name).
///
/// # Errors
/// I/O errors while spawning or talking to the process.
pub fn run_simple_with_env(
    package: &Package,
    input: &[u8],
    config: &Value,
    timeout: Duration,
    extra_env: &[(&str, &str)],
) -> std::io::Result<Report> {
    let (program, args) = package
        .command()
        .ok_or_else(|| std::io::Error::other("manifest has no runtime.command"))?;
    let env = InstanceEnv {
        plugin_id: &package.manifest.plugin.id,
        instance_id: "contract-test",
        plugin_dir: &package.dir,
        device: Some("test0"),
        vfo: None,
        freq_hz: Some(100_000_000),
        config,
    };
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(&package.dir)
        .envs(env.vars())
        .envs(extra_env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;

    let mut stdin = child.stdin.take();
    let input = input.to_vec();
    let writer = std::thread::spawn(move || {
        if let Some(stdin) = stdin.as_mut() {
            // A plugin that exits early closes the pipe; that is judged by the exit status.
            let _ = stdin.write_all(&input);
        }
        drop(stdin);
    });
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("no stdout"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("no stderr"))?;
    let stderr_reader = std::thread::spawn(move || {
        let mut text = String::new();
        let mut raw = Vec::new();
        if BufReader::new(stderr)
            .take(1 << 20)
            .read_to_end(&mut raw)
            .is_ok()
        {
            text = String::from_utf8_lossy(&raw).into_owned();
        }
        text
    });
    let validator = &package.validator;
    let started = Instant::now();
    let (events, heartbeats, violations, exit) = std::thread::scope(|scope| {
        let reader = scope.spawn(move || {
            let mut events = Vec::new();
            let mut heartbeats = 0;
            let mut violations = Vec::new();
            for line in LineReader::new(stdout) {
                match line {
                    Err(()) => {
                        violations.push((String::from("<oversized line>"), EventError::TooLong))
                    }
                    Ok(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => {}
                    Ok(bytes) => match validator.check_line(&bytes) {
                        Ok(Line::Event(event)) => events.push(event),
                        Ok(Line::Heartbeat) => heartbeats += 1,
                        Err(err) => violations.push((
                            String::from_utf8_lossy(&bytes).chars().take(200).collect(),
                            err,
                        )),
                    },
                }
            }
            (events, heartbeats, violations)
        });
        let exit = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if started.elapsed() > timeout => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(_) => break None,
            }
        };
        let (events, heartbeats, violations) = reader.join().unwrap_or_default();
        (events, heartbeats, violations, exit)
    });
    let _ = writer.join();
    let stderr = stderr_reader.join().unwrap_or_default();
    Ok(Report {
        events,
        heartbeats,
        violations,
        exit,
        stderr,
    })
}

/// Assert the generic contract rules on a clean run; returns every problem found.
#[must_use]
pub fn check(report: &Report) -> Vec<String> {
    let mut problems = Vec::new();
    for (line, err) in &report.violations {
        problems.push(format!("stdout line rejected ({err}): {line}"));
    }
    match report.exit {
        None => problems.push("did not exit after stdin was closed (timeout)".to_owned()),
        Some(status) if !status.success() => {
            problems.push(format!("exited with {status} after stdin was closed"));
        }
        Some(_) => {}
    }
    problems
}

/// Static checks of a plugin directory: manifest, schema and executable present.
///
/// # Errors
/// A description of every problem.
pub fn check_static(dir: &Path) -> Result<Package, Vec<String>> {
    let package = Package::load(dir).map_err(|err| vec![err.to_string()])?;
    let mut problems = Vec::new();
    if package.manifest.plugin.protocol != PROTOCOL_VERSION {
        problems.push("protocol mismatch".to_owned());
    }
    if let Some((program, _)) = package.command()
        && program.components().count() > 1
        && !program.exists()
    {
        problems.push(format!(
            "runtime.command[0] {} does not exist",
            program.display()
        ));
    }
    if problems.is_empty() {
        Ok(package)
    } else {
        Err(problems)
    }
}

/// Newline-separated records with a size cap; an oversized record is reported once as `Err(())`
/// and skipped without buffering it.
struct LineReader<R: Read> {
    inner: BufReader<R>,
}

impl<R: Read> LineReader<R> {
    fn new(inner: R) -> Self {
        Self {
            inner: BufReader::new(inner),
        }
    }
}

impl<R: Read> Iterator for LineReader<R> {
    type Item = Result<Vec<u8>, ()>;

    fn next(&mut self) -> Option<Self::Item> {
        let mut line = Vec::new();
        let mut oversized = false;
        loop {
            let chunk = match self.inner.fill_buf() {
                Ok([]) | Err(_) => break,
                Ok(chunk) => chunk,
            };
            let (take, done) = match chunk.iter().position(|&b| b == b'\n') {
                Some(pos) => (pos + 1, true),
                None => (chunk.len(), false),
            };
            if !oversized {
                line.extend_from_slice(&chunk[..take]);
                if line.len() > MAX_EVENT_LINE + 1 {
                    oversized = true;
                    line.clear();
                }
            }
            self.inner.consume(take);
            if done {
                return Some(if oversized {
                    Err(())
                } else {
                    line.pop();
                    Ok(line)
                });
            }
        }
        if oversized {
            Some(Err(()))
        } else if line.is_empty() {
            None
        } else {
            Some(Ok(line))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_reader_splits_and_caps() {
        let mut data = b"a\nbb\n".to_vec();
        data.extend(vec![b'x'; MAX_EVENT_LINE + 10]);
        data.extend_from_slice(b"\nlast");
        let lines: Vec<_> = LineReader::new(&data[..]).collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], Ok(b"a".to_vec()));
        assert_eq!(lines[1], Ok(b"bb".to_vec()));
        assert_eq!(lines[2], Err(()));
        assert_eq!(lines[3], Ok(b"last".to_vec()));
    }
}
