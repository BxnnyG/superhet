//! The weather gallery on disk: one directory per pass below the weather store, written by the
//! weather plugin (`pass.json`, images, thumbnails), read, annotated and trimmed here.
//!
//! Directories whose name starts with a dot (the plugin's work area) are never listed. Names
//! that arrive over HTTP are checked against the exact shapes the plugin writes before they are
//! joined to a path.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Largest `pass.json` read.
const MAX_DOC_BYTES: u64 = 1 << 20;

/// `<unix seconds>-<4 hex digits>`, the name of a pass directory.
#[must_use]
pub fn valid_id(id: &str) -> bool {
    let Some((secs, hex)) = id.split_once('-') else {
        return false;
    };
    (6..=12).contains(&secs.len())
        && secs.bytes().all(|b| b.is_ascii_digit())
        && hex.len() == 4
        && hex
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// A PNG file name as the plugin writes it: `[A-Za-z0-9._-]`, up to 96 characters, no leading dot.
#[must_use]
pub fn valid_image_name(name: &str) -> bool {
    name.len() <= 96
        && std::path::Path::new(name)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("png"))
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A pass found on disk.
#[derive(Debug, Clone)]
pub struct Capture {
    pub id: String,
    /// The parsed `pass.json`.
    pub doc: Value,
    /// Bytes of the whole directory.
    pub size_bytes: u64,
}

impl Capture {
    /// Start of the pass, Unix milliseconds.
    #[must_use]
    pub fn started_ms(&self) -> i64 {
        self.doc["started_ms"].as_i64().unwrap_or(0)
    }

    /// The device the pass was received on, if known.
    #[must_use]
    pub fn device(&self) -> Option<&str> {
        self.doc["device"].as_str()
    }

    /// Whether the user keeps the pass beyond the quota.
    #[must_use]
    pub fn keep(&self) -> bool {
        self.doc["keep"].as_bool().unwrap_or(false)
    }

    /// Whether `name` is one of the pass's images (or its thumbnail).
    #[must_use]
    pub fn has_file(&self, name: &str) -> bool {
        self.doc["images"].as_array().is_some_and(|images| {
            images.iter().any(|i| {
                i["name"].as_str() == Some(name) || i["thumb"].as_str().is_some_and(|t| t == name)
            })
        })
    }
}

/// Bytes below `path` (files only, symlinks not followed).
#[must_use]
pub fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path()),
            Ok(t) if t.is_file() => e.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

/// Read one pass; `None` when it does not exist or its document is unusable.
#[must_use]
pub fn read(store: &Path, id: &str) -> Option<Capture> {
    if !valid_id(id) {
        return None;
    }
    let dir = store.join(id);
    let file = dir.join("pass.json");
    if fs::metadata(&file).ok()?.len() > MAX_DOC_BYTES {
        return None;
    }
    let doc: Value = serde_json::from_slice(&fs::read(&file).ok()?).ok()?;
    if !doc.is_object() || doc["id"].as_str() != Some(id) {
        return None;
    }
    Some(Capture {
        id: id.to_owned(),
        doc,
        size_bytes: dir_size(&dir),
    })
}

/// Every pass in the store, newest first.
#[must_use]
pub fn scan(store: &Path) -> Vec<Capture> {
    let Ok(entries) = fs::read_dir(store) else {
        return Vec::new();
    };
    let mut out: Vec<Capture> = entries
        .flatten()
        .filter_map(|e| read(store, e.file_name().to_str()?))
        .collect();
    out.sort_by_key(|c| std::cmp::Reverse(c.started_ms()));
    out
}

/// Set top-level fields of `pass.json` (through a temporary file). `false` when the pass is gone
/// or its directory cannot be written.
pub fn set_fields(store: &Path, id: &str, fields: &[(&str, Value)]) -> bool {
    let Some(capture) = read(store, id) else {
        return false;
    };
    let mut doc = capture.doc;
    for (key, value) in fields {
        doc[*key] = value.clone();
    }
    let dir = store.join(id);
    let tmp = dir.join("pass.json.part");
    let Ok(text) = serde_json::to_vec_pretty(&doc) else {
        return false;
    };
    fs::write(&tmp, text).is_ok() && fs::rename(&tmp, dir.join("pass.json")).is_ok()
}

/// Delete a pass; `true` when something was removed.
pub fn delete(store: &Path, id: &str) -> bool {
    valid_id(id) && fs::remove_dir_all(store.join(id)).is_ok()
}

/// Path of a file of a pass, if the pass lists it.
#[must_use]
pub fn file_path(store: &Path, capture: &Capture, name: &str) -> Option<PathBuf> {
    (valid_image_name(name) && capture.has_file(name)).then(|| store.join(&capture.id).join(name))
}

/// Delete the oldest passes that are not kept until the store is under 90 % of `max_bytes`
/// (`0` = no limit). Returns the bytes in use afterwards and the ids removed.
pub fn enforce_quota(store: &Path, max_bytes: u64) -> (u64, Vec<String>) {
    let mut all = scan(store);
    let mut used: u64 = all.iter().map(|c| c.size_bytes).sum();
    let mut removed = Vec::new();
    if max_bytes == 0 || used <= max_bytes {
        return (used, removed);
    }
    let target = max_bytes / 10 * 9;
    // `scan` is newest first: take victims from the back.
    while used > target {
        let Some(pos) = all.iter().rposition(|c| !c.keep()) else {
            break;
        };
        let victim = all.remove(pos);
        if delete(store, &victim.id) {
            used = used.saturating_sub(victim.size_bytes);
            removed.push(victim.id);
        }
    }
    (used, removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn scratch(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("superhet-gallery-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    fn pass(store: &Path, id: &str, started_s: i64, bytes: usize, keep: bool) {
        let dir = store.join(id);
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(dir.join("img.png"), vec![0u8; bytes]).expect("img");
        fs::write(dir.join("thumb-img.png"), b"t").expect("thumb");
        let doc = json!({"v": 1, "id": id, "started_ms": started_s * 1000, "keep": keep,
            "device": "dev0", "images": [{"name": "img.png", "thumb": "thumb-img.png"}]});
        fs::write(dir.join("pass.json"), doc.to_string()).expect("doc");
    }

    #[test]
    fn names_from_the_network_are_checked() {
        assert!(valid_id("1795436348-ab12"));
        assert!(!valid_id("1795436348-AB12"));
        assert!(!valid_id("../etc"));
        assert!(!valid_id("1795436348-ab123"));
        assert!(!valid_id("17954-ab12"));
        assert!(!valid_id("1795436348_ab12"));
        assert!(valid_image_name("msu_mr_rgb-124.png"));
        assert!(valid_image_name("thumb-a.PNG"));
        for bad in [
            "a/b.png",
            "..png",
            ".hidden.png",
            "a.jpg",
            "a b.png",
            "",
            &"a".repeat(100),
        ] {
            assert!(!valid_image_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn scanning_lists_newest_first_and_skips_the_work_area_and_junk() {
        let store = scratch("scan");
        pass(&store, "1700000100-0001", 1_700_000_100, 10, false);
        pass(&store, "1700000300-0002", 1_700_000_300, 10, false);
        fs::create_dir_all(store.join(".work")).expect("work");
        fs::write(store.join(".work/pass.json"), "{}").expect("x");
        fs::create_dir_all(store.join("1700000500-0003")).expect("empty");
        fs::create_dir_all(store.join("1700000600-0004")).expect("bad doc");
        fs::write(store.join("1700000600-0004/pass.json"), "not json").expect("x");
        // A document whose id does not match its directory is not trusted.
        fs::create_dir_all(store.join("1700000700-0005")).expect("liar");
        fs::write(
            store.join("1700000700-0005/pass.json"),
            json!({"id": "1700000100-0001"}).to_string(),
        )
        .expect("x");
        let ids: Vec<String> = scan(&store).into_iter().map(|c| c.id).collect();
        assert_eq!(ids, ["1700000300-0002", "1700000100-0001"]);
        assert!(scan(&store.join("missing")).is_empty());
        let _ = fs::remove_dir_all(&store);
    }

    #[test]
    fn fields_are_updated_in_place_and_files_must_be_listed() {
        let store = scratch("fields");
        pass(&store, "1700000100-0001", 1_700_000_100, 10, false);
        assert!(set_fields(
            &store,
            "1700000100-0001",
            &[("keep", json!(true)), ("max_elevation_deg", json!(41.5))]
        ));
        let c = read(&store, "1700000100-0001").expect("read");
        assert!(c.keep());
        assert_eq!(c.doc["max_elevation_deg"], 41.5);
        assert_eq!(c.doc["images"][0]["name"], "img.png", "other fields stay");
        assert!(!set_fields(
            &store,
            "1700000999-0009",
            &[("keep", json!(true))]
        ));
        assert!(file_path(&store, &c, "img.png").is_some());
        assert!(file_path(&store, &c, "thumb-img.png").is_some());
        assert!(file_path(&store, &c, "pass.json").is_none());
        assert!(file_path(&store, &c, "other.png").is_none());
        assert!(delete(&store, "1700000100-0001"));
        assert!(!delete(&store, "1700000100-0001"));
        assert!(!delete(&store, "../x"));
        let _ = fs::remove_dir_all(&store);
    }

    #[test]
    fn the_quota_removes_the_oldest_passes_that_are_not_kept() {
        let store = scratch("quota");
        pass(&store, "1700000100-0001", 1_700_000_100, 1000, true); // oldest, kept
        pass(&store, "1700000200-0002", 1_700_000_200, 1000, false);
        pass(&store, "1700000300-0003", 1_700_000_300, 1000, false);
        pass(&store, "1700000400-0004", 1_700_000_400, 1000, false); // newest
        let each = read(&store, "1700000400-0004").expect("read").size_bytes;
        // Room for three of them (limit 3.5, target 3.15): one must go, and it is not the kept one.
        let max = each * 7 / 2;
        let (used, removed) = enforce_quota(&store, max);
        assert_eq!(removed, ["1700000200-0002"]);
        assert!(used <= max / 10 * 9 && used >= each * 2, "{used}");
        // Nothing to do when under the limit, or with no limit.
        assert!(enforce_quota(&store, max).1.is_empty());
        assert!(enforce_quota(&store, 0).1.is_empty());
        // Only kept passes left over the limit: nothing more can be removed.
        let (_, removed) = enforce_quota(&store, 1);
        assert_eq!(removed, ["1700000300-0003", "1700000400-0004"]);
        assert!(read(&store, "1700000100-0001").is_some());
        let _ = fs::remove_dir_all(&store);
    }
}
