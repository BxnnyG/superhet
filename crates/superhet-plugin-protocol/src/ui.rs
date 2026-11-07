//! Rules for the static assets of a plugin's web panel (`[ui]`, ADR-0022).
//!
//! A panel is a small web page the plugin ships in a subdirectory of its package (the parent
//! directory of `ui.entry`, the "UI root"). The API serves exactly the files below that root
//! (`/api/v1/plugin-ui/<plugin id>/<path below the root>`) after asking the plugin host for them;
//! these helpers are shared by the manifest check, the host and the API so that all three agree
//! on what a valid asset path is and which file types are served.

/// Largest asset the host hands out (bytes). The bus payload limit is 1 MiB, the reply carries
/// a small header as well.
pub const MAX_ASSET_BYTES: usize = 640 * 1024;
/// Longest asset path (bytes).
pub const MAX_PATH_BYTES: usize = 200;
/// Deepest asset path (segments).
pub const MAX_PATH_SEGMENTS: usize = 8;
/// Longest panel title (characters).
pub const MAX_TITLE_CHARS: usize = 32;
/// Most plugin ids a panel may read.
pub const MAX_READS: usize = 8;

/// Whether `path` names a file below the UI root: `/`-separated segments of `[A-Za-z0-9._-]`,
/// none empty, none starting with a dot (so no `.`, `..` and no hidden files), at most
/// [`MAX_PATH_SEGMENTS`] deep and [`MAX_PATH_BYTES`] long. Everything else (backslashes,
/// percent signs, NUL, absolute paths, drive letters) is refused outright instead of normalised.
#[must_use]
pub fn valid_asset_path(path: &str) -> bool {
    if path.is_empty() || path.len() > MAX_PATH_BYTES {
        return false;
    }
    let mut segments = 0;
    for segment in path.split('/') {
        segments += 1;
        if segments > MAX_PATH_SEGMENTS
            || segment.is_empty()
            || segment.starts_with('.')
            || !segment
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            return false;
        }
    }
    true
}

/// Split a manifest `ui.entry` (`ui/index.html`) into the UI root (`ui`) and the page below it
/// (`index.html`). `None` unless the entry is a valid asset path of an `.html` file in a
/// subdirectory: a page at the top of the package would make the whole package (binaries,
/// configuration) the UI root.
#[must_use]
pub fn split_entry(entry: &str) -> Option<(&str, &str)> {
    if !valid_asset_path(entry) || mime_for(entry) != Some(HTML) {
        return None;
    }
    let (root, page) = entry.rsplit_once('/')?;
    Some((root, page))
}

const HTML: &str = "text/html; charset=utf-8";

/// Content type of a servable asset, by (lower-cased) file extension; `None` for every type the
/// panel route does not serve. Scripts, styles, images and fonts only: no WebAssembly, no
/// source maps, no archives.
#[must_use]
pub fn mime_for(path: &str) -> Option<&'static str> {
    let (_, ext) = path.rsplit_once('.')?;
    Some(match ext.to_ascii_lowercase().as_str() {
        "html" => HTML,
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "txt" => "text/plain; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "woff2" => "font/woff2",
        _ => return None,
    })
}

/// A valid panel title: 1..=[`MAX_TITLE_CHARS`] characters, no control characters.
#[must_use]
pub fn valid_title(title: &str) -> bool {
    let chars = title.chars().count();
    (1..=MAX_TITLE_CHARS).contains(&chars)
        && !title.chars().any(char::is_control)
        && title.trim() == title
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_must_stay_plain_and_below_the_root() {
        for good in ["index.html", "js/app.js", "a/b/c/d.css", "my-file_1.min.js"] {
            assert!(valid_asset_path(good), "{good}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "../plugin.toml",
            "ui/../plugin.toml",
            "a/./b",
            "a//b",
            "a/",
            ".hidden",
            "a/.env",
            "a\\b",
            "a%2fb",
            "%2e%2e/x",
            "a\0b",
            "C:/x",
            "a b",
            "ä.js",
            "1/2/3/4/5/6/7/8/9.js",
        ] {
            assert!(!valid_asset_path(bad), "{bad:?}");
        }
        assert!(!valid_asset_path(&"a".repeat(MAX_PATH_BYTES + 1)));
    }

    #[test]
    fn the_entry_needs_a_directory_of_its_own() {
        assert_eq!(split_entry("ui/index.html"), Some(("ui", "index.html")));
        assert_eq!(split_entry("web/ui/p.html"), Some(("web/ui", "p.html")));
        // Top-level page: the UI root would be the whole package.
        assert_eq!(split_entry("index.html"), None);
        assert_eq!(split_entry("ui/app.js"), None);
        assert_eq!(split_entry("../ui/index.html"), None);
        assert_eq!(split_entry("ui/"), None);
    }

    #[test]
    fn only_web_assets_are_served() {
        assert_eq!(mime_for("a.js"), Some("text/javascript; charset=utf-8"));
        assert_eq!(mime_for("A.CSS"), Some("text/css; charset=utf-8"));
        assert_eq!(mime_for("x.svg"), Some("image/svg+xml"));
        for none in [
            "x",
            "x.wasm",
            "x.map",
            "x.exe",
            "x.toml",
            "x.py",
            "x.html.bak",
        ] {
            assert_eq!(mime_for(none), None, "{none}");
        }
    }

    #[test]
    fn titles_are_short_plain_text() {
        assert!(valid_title("Echo view"));
        assert!(!valid_title(""));
        assert!(!valid_title(" padded"));
        assert!(!valid_title("new\nline"));
        assert!(!valid_title(&"x".repeat(MAX_TITLE_CHARS + 1)));
    }
}
