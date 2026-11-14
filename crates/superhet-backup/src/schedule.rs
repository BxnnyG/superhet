//! Scheduled backups: the schedule syntax, writing an archive into a directory, rotation.
//!
//! Schedule (`SUPERHET_BACKUP_SCHEDULE`, always UTC): `off`, `hourly`, `daily [HH:MM]` (default
//! 03:30), `weekly [mon|tue|...|sun] [HH:MM]` (default mon 03:30).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::Serialize;
use sqlx::PgPool;

use crate::{Error, ExportOptions, crypto, export};

const PREFIX: &str = "superhet-backup-";

/// When scheduled backups run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schedule {
    /// Never.
    Off,
    /// At the start of every hour.
    Hourly,
    /// Every day at a minute of the day (UTC).
    Daily { minute: u32 },
    /// Every week on a weekday (0 = Monday) at a minute of the day (UTC).
    Weekly { weekday: u32, minute: u32 },
}

fn parse_time(text: &str) -> Result<u32, String> {
    let (h, m) = text
        .split_once(':')
        .ok_or_else(|| format!("{text:?} is not HH:MM"))?;
    let h: u32 = h.parse().map_err(|_| format!("{text:?} is not HH:MM"))?;
    let m: u32 = m.parse().map_err(|_| format!("{text:?} is not HH:MM"))?;
    if h > 23 || m > 59 {
        return Err(format!("{text:?} is not a time of day"));
    }
    Ok(h * 60 + m)
}

/// Parse a schedule setting.
///
/// # Errors
/// A message naming what is wrong.
pub fn parse(text: &str) -> Result<Schedule, String> {
    const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    let words: Vec<String> = text
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect();
    let words: Vec<&str> = words.iter().map(String::as_str).collect();
    match words.as_slice() {
        [] | ["off" | "none" | "disabled"] => Ok(Schedule::Off),
        ["hourly"] => Ok(Schedule::Hourly),
        ["daily"] => Ok(Schedule::Daily {
            minute: 3 * 60 + 30,
        }),
        ["daily", time] => Ok(Schedule::Daily {
            minute: parse_time(time)?,
        }),
        ["weekly", rest @ ..] if rest.len() <= 2 => {
            let (mut weekday, mut minute) = (0, 3 * 60 + 30);
            for word in rest {
                if let Some(i) = DAYS.iter().position(|d| d == word) {
                    weekday = u32::try_from(i).unwrap_or(0);
                } else {
                    minute = parse_time(word)?;
                }
            }
            Ok(Schedule::Weekly { weekday, minute })
        }
        _ => Err(format!(
            "unknown schedule {text:?} (use off, hourly, \"daily 03:30\" or \"weekly mon 03:30\")"
        )),
    }
}

impl Schedule {
    /// The first run strictly after `now` (unix seconds), `None` for [`Schedule::Off`].
    #[must_use]
    pub fn next_after(self, now: i64) -> Option<i64> {
        let day = now.div_euclid(86_400);
        match self {
            Self::Off => None,
            Self::Hourly => Some((now.div_euclid(3600) + 1) * 3600),
            Self::Daily { minute } => {
                let at = day * 86_400 + i64::from(minute) * 60;
                Some(if at > now { at } else { at + 86_400 })
            }
            Self::Weekly { weekday, minute } => (0..=7).find_map(|d| {
                let candidate_day = day + d;
                // 1970-01-01 was a Thursday (3 with Monday = 0).
                let wd = (candidate_day + 3).rem_euclid(7);
                let at = candidate_day * 86_400 + i64::from(minute) * 60;
                (wd == i64::from(weekday) && at > now).then_some(at)
            }),
        }
    }
}

/// UTC calendar date and time of a unix timestamp.
fn civil(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = u32::try_from(secs.rem_euclid(86_400)).unwrap_or(0);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// `superhet-backup-YYYYMMDD-HHMMSS.zip` (plus `.enc` when sealed).
#[must_use]
pub fn file_name(now: i64, encrypted: bool) -> String {
    let (y, mo, d, h, mi, s) = civil(now);
    let ext = if encrypted { "zip.enc" } else { "zip" };
    format!("{PREFIX}{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}.{ext}")
}

/// A backup file in the target directory.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Stored {
    /// File name.
    pub name: String,
    /// Size in bytes.
    pub bytes: u64,
}

/// The backup files in `dir`, newest first (the names sort by time).
///
/// # Errors
/// Fails if the directory cannot be read.
pub fn list(dir: &Path) -> io::Result<Vec<Stored>> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(PREFIX) && (name.ends_with(".zip") || name.ends_with(".zip.enc")) {
            out.push(Stored {
                name,
                bytes: entry.metadata()?.len(),
            });
        }
    }
    out.sort_by(|a, b| b.name.cmp(&a.name));
    Ok(out)
}

/// Delete all but the newest `keep` backups; returns the removed names.
///
/// # Errors
/// Fails if the directory cannot be read or a file cannot be removed.
pub fn rotate(dir: &Path, keep: usize) -> io::Result<Vec<String>> {
    let mut removed = Vec::new();
    for old in list(dir)?.into_iter().skip(keep.max(1)) {
        fs::remove_file(dir.join(&old.name))?;
        removed.push(old.name);
    }
    Ok(removed)
}

/// Settings of one scheduled backup run.
#[derive(Debug, Clone)]
pub struct RunOptions {
    /// Target directory.
    pub dir: PathBuf,
    /// How many archives to keep.
    pub keep: usize,
    /// Seal the archive with this passphrase.
    pub passphrase: Option<String>,
    /// Include credentials (password hashes, secrets).
    pub include_credentials: bool,
    /// Add the audit log and decoder history.
    pub include_history: bool,
}

/// Write one archive into the target directory and rotate. The file appears atomically (written
/// as `.partial`, then renamed) with mode 0600.
///
/// # Errors
/// Database or I/O failures.
pub async fn run_once(pool: &PgPool, now: i64, options: &RunOptions) -> Result<Stored, Error> {
    let mut areas: Vec<String> = crate::default_areas()
        .into_iter()
        .map(str::to_owned)
        .collect();
    if options.include_history {
        areas.extend(["audit".to_owned(), "decoder_events".to_owned()]);
    }
    let (mut archive, _) = export(
        pool,
        &ExportOptions {
            areas,
            include_credentials: options.include_credentials,
            created_by: "scheduled".into(),
            recordings_dir: None,
        },
    )
    .await?;
    fs::create_dir_all(&options.dir)?;
    let name = file_name(now, options.passphrase.is_some());
    let partial = options.dir.join(format!("{name}.partial"));
    let result = (|| -> Result<u64, Error> {
        let mut out = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&partial)?;
        match &options.passphrase {
            Some(pass) => crypto::encrypt(&mut archive, &mut out, pass)?,
            None => {
                io::copy(&mut archive, &mut out)?;
            }
        }
        out.flush()?;
        out.sync_all()?;
        Ok(File::open(&partial)?.metadata()?.len())
    })();
    match result {
        Ok(bytes) => {
            fs::rename(&partial, options.dir.join(&name))?;
            rotate(&options.dir, options.keep)?;
            Ok(Stored { name, bytes })
        }
        Err(e) => {
            let _ = fs::remove_file(&partial);
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedules_parse() {
        assert_eq!(parse("").expect("p"), Schedule::Off);
        assert_eq!(parse("OFF").expect("p"), Schedule::Off);
        assert_eq!(parse("hourly").expect("p"), Schedule::Hourly);
        assert_eq!(parse("daily").expect("p"), Schedule::Daily { minute: 210 });
        assert_eq!(
            parse("daily 04:05").expect("p"),
            Schedule::Daily { minute: 245 }
        );
        assert_eq!(
            parse("weekly sun 23:59").expect("p"),
            Schedule::Weekly {
                weekday: 6,
                minute: 1439
            }
        );
        assert_eq!(
            parse("weekly").expect("p"),
            Schedule::Weekly {
                weekday: 0,
                minute: 210
            }
        );
        for bad in [
            "daily 25:00",
            "monthly",
            "weekly funday",
            "daily 1:2:3",
            "hourly now",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn next_runs() {
        // 2026-11-11 12:00:00 UTC is a Wednesday.
        let now = 1_794_398_400;
        assert_eq!(civil(now), (2026, 11, 11, 12, 0, 0));
        assert_eq!(Schedule::Hourly.next_after(now), Some(now + 3600));
        assert_eq!(
            Schedule::Daily {
                minute: 12 * 60 + 1
            }
            .next_after(now),
            Some(now + 60)
        );
        assert_eq!(
            Schedule::Daily { minute: 12 * 60 }.next_after(now),
            Some(now + 86_400)
        );
        // Monday 03:30 after Wednesday noon is 5 days minus 8.5 hours away.
        assert_eq!(
            Schedule::Weekly {
                weekday: 0,
                minute: 210
            }
            .next_after(now),
            Some(now + 5 * 86_400 - 8 * 3600 - 1800)
        );
        assert_eq!(Schedule::Off.next_after(now), None);
    }

    #[test]
    fn names_sort_by_time_and_rotation_keeps_the_newest() {
        assert_eq!(file_name(0, false), "superhet-backup-19700101-000000.zip");
        assert_eq!(
            file_name(1_794_398_400, true),
            "superhet-backup-20261111-120000.zip.enc"
        );
        let dir = std::env::temp_dir().join(format!("sh-rot-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("dir");
        for t in [100, 200, 300, 400] {
            fs::write(dir.join(file_name(t * 86_400, t % 200 == 0)), b"x").expect("write");
        }
        fs::write(dir.join("unrelated.txt"), b"x").expect("write");
        let removed = rotate(&dir, 2).expect("rotate");
        assert_eq!(removed.len(), 2);
        let left: Vec<String> = list(&dir)
            .expect("list")
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(
            left,
            vec![
                file_name(400 * 86_400, true),
                file_name(300 * 86_400, false)
            ]
        );
        assert!(dir.join("unrelated.txt").exists());
        fs::remove_dir_all(&dir).expect("cleanup");
    }
}
