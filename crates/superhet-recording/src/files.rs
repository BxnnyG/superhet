//! Where recordings live on disk, and the rules that keep every access inside that directory.
//!
//! A recording's file name is never taken from a client: the database stores a *stem* made of
//! `[A-Za-z0-9_-]` that the service generated, and the path is rebuilt from the root, the kind
//! directory and the stem on every access. [`resolve`] refuses anything else, so a corrupted
//! row cannot reach outside the recordings directory.

use std::path::{Path, PathBuf};

/// Longest stem accepted.
pub const MAX_STEM_LEN: usize = 96;

/// What a recording contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Opus in Ogg.
    Audio,
    /// SigMF IQ (`.sigmf-data` + `.sigmf-meta`).
    Iq,
}

/// One file of a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The audio file or the IQ samples.
    Main,
    /// The SigMF metadata of an IQ recording.
    Meta,
}

impl Kind {
    /// Parse the database spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "audio" => Some(Self::Audio),
            "iq" => Some(Self::Iq),
            _ => None,
        }
    }

    /// Database spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Audio => "audio",
            Self::Iq => "iq",
        }
    }

    fn dir(self) -> &'static str {
        self.as_str()
    }

    /// File extension of a part; `None` where the kind has no such part.
    #[must_use]
    pub fn extension(self, part: Part) -> Option<&'static str> {
        match (self, part) {
            (Self::Audio, Part::Main) => Some("ogg"),
            (Self::Iq, Part::Main) => Some("sigmf-data"),
            (Self::Iq, Part::Meta) => Some("sigmf-meta"),
            (Self::Audio, Part::Meta) => None,
        }
    }

    /// Media type served for a part.
    #[must_use]
    pub fn mime(self, part: Part) -> &'static str {
        match (self, part) {
            (Self::Audio, _) => "audio/ogg",
            (Self::Iq, Part::Meta) => "application/json",
            (Self::Iq, Part::Main) => "application/octet-stream",
        }
    }
}

/// Whether `stem` is one the service could have generated.
#[must_use]
pub fn is_safe_stem(stem: &str) -> bool {
    !stem.is_empty()
        && stem.len() <= MAX_STEM_LEN
        && stem
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The stem of a new recording: id and frequency, nothing user-controlled.
#[must_use]
pub fn make_stem(id: i64, freq_hz: Option<i64>) -> String {
    match freq_hz {
        Some(f) if f > 0 => format!("rec{id}-{f}hz"),
        _ => format!("rec{id}"),
    }
}

/// The path of a recording's file under `root`, or `None` when the stem is not safe or the kind
/// has no such part.
#[must_use]
pub fn resolve(root: &Path, kind: Kind, stem: &str, part: Part) -> Option<PathBuf> {
    if !is_safe_stem(stem) {
        return None;
    }
    let ext = kind.extension(part)?;
    Some(root.join(kind.dir()).join(format!("{stem}.{ext}")))
}

/// A file name for `Content-Disposition` made from a free-text title: ASCII letters, digits,
/// dots, dashes and underscores only, never empty, never starting with a dot.
#[must_use]
pub fn download_name(title: &str, ext: &str) -> String {
    let mut base: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(60)
        .collect();
    base = base.trim_matches('_').to_owned();
    if base.is_empty() {
        base = "recording".to_owned();
    }
    format!("{base}.{ext}")
}

/// Total size in bytes of the regular files directly in `dir` whose stem starts with `stem`
/// (the data and metadata file of an IQ recording). Missing files count as 0.
#[must_use]
pub fn size_on_disk(root: &Path, kind: Kind, stem: &str) -> u64 {
    [Part::Main, Part::Meta]
        .into_iter()
        .filter_map(|part| resolve(root, kind, stem, part))
        .filter_map(|path| std::fs::metadata(path).ok())
        .filter(std::fs::Metadata::is_file)
        .map(|m| m.len())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_generated_stems_resolve() {
        let root = Path::new("/data/recordings");
        assert_eq!(
            resolve(root, Kind::Audio, "rec7-145500000hz", Part::Main),
            Some(PathBuf::from("/data/recordings/audio/rec7-145500000hz.ogg"))
        );
        assert_eq!(
            resolve(root, Kind::Iq, "rec9", Part::Meta),
            Some(PathBuf::from("/data/recordings/iq/rec9.sigmf-meta"))
        );
        assert_eq!(resolve(root, Kind::Audio, "rec9", Part::Meta), None);
    }

    #[test]
    fn path_traversal_is_refused() {
        let root = Path::new("/data/recordings");
        for bad in [
            "..",
            "../etc/passwd",
            "../../x",
            "a/b",
            "a\\b",
            "/etc/passwd",
            ".hidden",
            "a.b",
            "rec1\0",
            "rec1%2f..",
            "%2e%2e",
            "",
            &"a".repeat(MAX_STEM_LEN + 1),
        ] {
            assert!(!is_safe_stem(bad), "{bad:?}");
            assert_eq!(resolve(root, Kind::Audio, bad, Part::Main), None, "{bad:?}");
            assert_eq!(resolve(root, Kind::Iq, bad, Part::Meta), None, "{bad:?}");
        }
    }

    #[test]
    fn generated_stems_are_safe() {
        for stem in [
            make_stem(1, None),
            make_stem(42, Some(145_500_000)),
            make_stem(3, Some(-5)),
        ] {
            assert!(is_safe_stem(&stem), "{stem}");
        }
    }

    #[test]
    fn download_names_are_plain_ascii() {
        assert_eq!(
            download_name("Airband \"Tower\" 118.3", "ogg"),
            "Airband__Tower__118_3.ogg"
        );
        assert_eq!(download_name("../../etc/passwd", "ogg"), "etc_passwd.ogg");
        assert_eq!(download_name("", "ogg"), "recording.ogg");
        assert_eq!(download_name("\u{e4}\u{f6}\u{fc}", "ogg"), "recording.ogg");
        assert!(download_name(&"x".repeat(500), "ogg").len() <= 64);
    }

    #[test]
    fn kinds_round_trip() {
        for k in [Kind::Audio, Kind::Iq] {
            assert_eq!(Kind::parse(k.as_str()), Some(k));
        }
        assert_eq!(Kind::parse("video"), None);
    }
}
