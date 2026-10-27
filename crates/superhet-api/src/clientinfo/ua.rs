//! A small, dependency-free `User-Agent` parser: browser, version, operating system and device
//! class. It knows the common browsers and tools; anything else comes back as unknown rather than
//! guessed. User agents are client-supplied text, so this is for display, never for decisions.

use serde::Serialize;

/// What could be read from a user agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Default)]
pub struct UaInfo {
    /// `Chrome`, `Firefox`, `Safari`, `Edge`, `curl`, ...
    pub browser: Option<String>,
    /// Major version of the browser.
    pub browser_version: Option<String>,
    /// `Windows`, `macOS`, `Linux`, `Android`, `iOS`, `iPadOS`, `ChromeOS`.
    pub os: Option<String>,
    /// OS version where the string carries a trustworthy one.
    pub os_version: Option<String>,
    /// `desktop`, `mobile`, `tablet`, `bot`, `tool` or `unknown`.
    pub device: &'static str,
}

/// Text after `marker` up to the next `.`, `;`, space or `)`; the major version.
fn major_after<'a>(ua: &'a str, marker: &str) -> Option<&'a str> {
    let rest = &ua[ua.find(marker)? + marker.len()..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// Dotted or underscored version after `marker`, as `a.b`.
fn dotted_after(ua: &str, marker: &str) -> Option<String> {
    let rest = &ua[ua.find(marker)? + marker.len()..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '_'))
        .unwrap_or(rest.len());
    let v = rest[..end].replace('_', ".");
    let v = v.trim_matches('.');
    (!v.is_empty()).then(|| v.to_owned())
}

fn browser(ua: &str) -> (Option<&'static str>, Option<String>) {
    // Order matters: most browsers also claim to be Chrome, Safari and Mozilla.
    const MARKERS: &[(&str, &str)] = &[
        ("EdgA/", "Edge"),
        ("EdgiOS/", "Edge"),
        ("Edge/", "Edge"),
        ("Edg/", "Edge"),
        ("OPR/", "Opera"),
        ("OPiOS/", "Opera"),
        ("SamsungBrowser/", "Samsung Internet"),
        ("Vivaldi/", "Vivaldi"),
        ("YaBrowser/", "Yandex"),
        ("DuckDuckGo/", "DuckDuckGo"),
        ("FxiOS/", "Firefox"),
        ("Firefox/", "Firefox"),
        ("CriOS/", "Chrome"),
        ("Chromium/", "Chromium"),
        ("HeadlessChrome/", "Headless Chrome"),
        ("Chrome/", "Chrome"),
    ];
    for (marker, name) in MARKERS {
        if ua.contains(marker) {
            return (Some(name), major_after(ua, marker).map(str::to_owned));
        }
    }
    if ua.contains("Safari/") && ua.contains("Version/") {
        return (
            Some("Safari"),
            major_after(ua, "Version/").map(str::to_owned),
        );
    }
    if ua.contains("AppleWebKit/") {
        // WebView of an app on iOS or macOS: no Safari/Version tokens.
        return (
            Some("WebKit"),
            major_after(ua, "AppleWebKit/").map(str::to_owned),
        );
    }
    (None, None)
}

fn tool(ua: &str) -> Option<(&'static str, Option<String>)> {
    const TOOLS: &[(&str, &str)] = &[
        ("curl/", "curl"),
        ("Wget/", "Wget"),
        ("python-requests/", "python-requests"),
        ("python-httpx/", "python-httpx"),
        ("Go-http-client/", "Go-http-client"),
        ("okhttp/", "okhttp"),
        ("PostmanRuntime/", "Postman"),
        ("insomnia/", "Insomnia"),
        ("axios/", "axios"),
        ("node-fetch/", "node-fetch"),
        ("Playwright/", "Playwright"),
    ];
    TOOLS
        .iter()
        .find(|(marker, _)| ua.contains(marker))
        .map(|(marker, name)| (*name, major_after(ua, marker).map(str::to_owned)))
}

fn is_bot(ua: &str) -> bool {
    let l = ua.to_ascii_lowercase();
    ["bot", "crawler", "spider", "slurp", "monitor", "uptime"]
        .iter()
        .any(|m| l.contains(m))
}

fn os(ua: &str) -> (Option<&'static str>, Option<String>) {
    if ua.contains("iPhone") || ua.contains("iPod") {
        return (Some("iOS"), dotted_after(ua, "OS "));
    }
    if ua.contains("iPad") {
        return (Some("iPadOS"), dotted_after(ua, "OS "));
    }
    if ua.contains("Android") {
        return (Some("Android"), dotted_after(ua, "Android "));
    }
    if ua.contains("Windows") {
        let version = match dotted_after(ua, "Windows NT ").as_deref() {
            // Windows 11 still says NT 10.0.
            Some("10.0") => Some("10/11".to_owned()),
            Some("6.3") => Some("8.1".to_owned()),
            Some("6.2") => Some("8".to_owned()),
            Some("6.1") => Some("7".to_owned()),
            _ => None,
        };
        return (Some("Windows"), version);
    }
    if ua.contains("CrOS") {
        return (Some("ChromeOS"), None);
    }
    if ua.contains("Mac OS X") || ua.contains("Macintosh") {
        // Browsers freeze the reported version at 10.15.7: only newer-looking ones are shown.
        let v = dotted_after(ua, "Mac OS X ").filter(|v| v != "10.15.7");
        return (Some("macOS"), v);
    }
    if ua.contains("Linux") || ua.contains("X11") {
        return (Some("Linux"), None);
    }
    (None, None)
}

/// Parse a user agent string.
#[must_use]
pub fn parse(ua: &str) -> UaInfo {
    let ua = ua.trim();
    if ua.is_empty() {
        return UaInfo {
            device: "unknown",
            ..UaInfo::default()
        };
    }
    if let Some((name, version)) = tool(ua) {
        return UaInfo {
            browser: Some(name.to_owned()),
            browser_version: version,
            device: "tool",
            ..UaInfo::default()
        };
    }
    let (b, bv) = browser(ua);
    let (o, ov) = os(ua);
    let device = if is_bot(ua) {
        "bot"
    } else if ua.contains("iPad")
        || ua.contains("Tablet")
        || ua.contains("Tab ")
        || (ua.contains("Android") && !ua.contains("Mobile"))
    {
        "tablet"
    } else if ua.contains("Mobile") || ua.contains("iPhone") || ua.contains("iPod") {
        "mobile"
    } else if o.is_some() || b.is_some() {
        "desktop"
    } else {
        "unknown"
    };
    UaInfo {
        browser: b.map(str::to_owned),
        browser_version: bv,
        os: o.map(str::to_owned),
        os_version: ov,
        device,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(ua: &str) -> (String, String, String, &'static str) {
        let i = parse(ua);
        (
            format!(
                "{} {}",
                i.browser.unwrap_or_default(),
                i.browser_version.unwrap_or_default()
            )
            .trim()
            .to_owned(),
            format!(
                "{} {}",
                i.os.unwrap_or_default(),
                i.os_version.unwrap_or_default()
            )
            .trim()
            .to_owned(),
            String::new(),
            i.device,
        )
    }

    #[test]
    fn desktop_browsers() {
        let chrome_linux = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
        let r = p(chrome_linux);
        assert_eq!(
            (r.0.as_str(), r.1.as_str(), r.3),
            ("Chrome 131", "Linux", "desktop")
        );
        let firefox_win =
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:133.0) Gecko/20100101 Firefox/133.0";
        let r = p(firefox_win);
        assert_eq!(
            (r.0.as_str(), r.1.as_str(), r.3),
            ("Firefox 133", "Windows 10/11", "desktop")
        );
        let edge = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Edg/130.0.2849.80";
        assert_eq!(p(edge).0, "Edge 130");
        let opera = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 OPR/115.0.0.0";
        assert_eq!(p(opera).0, "Opera 115");
        let safari = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.2 Safari/605.1.15";
        let r = p(safari);
        assert_eq!(
            (r.0.as_str(), r.1.as_str(), r.3),
            ("Safari 17", "macOS", "desktop")
        );
    }

    #[test]
    fn mobile_and_tablets() {
        let iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_2 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.2 Mobile/15E148 Safari/604.1";
        let r = p(iphone);
        assert_eq!(
            (r.0.as_str(), r.1.as_str(), r.3),
            ("Safari 17", "iOS 17.2", "mobile")
        );
        let ipad = "Mozilla/5.0 (iPad; CPU OS 16_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/16.6 Mobile/15E148 Safari/604.1";
        let r = p(ipad);
        assert_eq!((r.1.as_str(), r.3), ("iPadOS 16.6", "tablet"));
        let pixel = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36";
        let r = p(pixel);
        assert_eq!(
            (r.0.as_str(), r.1.as_str(), r.3),
            ("Chrome 131", "Android 14", "mobile")
        );
        let android_tablet = "Mozilla/5.0 (Linux; Android 13; SM-X700) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
        assert_eq!(p(android_tablet).3, "tablet");
        let samsung = "Mozilla/5.0 (Linux; Android 14; SM-S918B) AppleWebKit/537.36 (KHTML, like Gecko) SamsungBrowser/25.0 Chrome/121.0.0.0 Mobile Safari/537.36";
        assert_eq!(p(samsung).0, "Samsung Internet 25");
        let ff_android = "Mozilla/5.0 (Android 14; Mobile; rv:133.0) Gecko/133.0 Firefox/133.0";
        assert_eq!(p(ff_android).3, "mobile");
    }

    #[test]
    fn tools_and_bots() {
        let r = p("curl/8.5.0");
        assert_eq!((r.0.as_str(), r.3), ("curl 8", "tool"));
        assert_eq!(p("python-requests/2.31.0").3, "tool");
        let bot = "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)";
        assert_eq!(p(bot).3, "bot");
        let headless = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) HeadlessChrome/131.0.0.0 Safari/537.36";
        assert_eq!(p(headless).0, "Headless Chrome 131");
    }

    #[test]
    fn unknown_and_empty_stay_unknown() {
        assert_eq!(parse("").device, "unknown");
        let i = parse("totally-custom-client");
        assert_eq!((i.browser, i.os, i.device), (None, None, "unknown"));
    }
}
