//! Help for the most common OIDC mistake: an endpoint URL (authorization, token, discovery
//! document) entered where the issuer belongs. Pure string work, no network; the probing that
//! confirms a suggestion lives in `diagnose.rs`. Never rewrites anything by itself.

use openidconnect::url::Url;

/// Upper bound of discovery attempts when looking for the right issuer.
pub const MAX_CANDIDATES: usize = 6;

/// Paths that belong to a provider endpoint, not to an issuer, with a name for the message.
/// The `bool` says whether stripping the suffix gives the issuer (it does not for Authentik,
/// whose endpoints are shared by all applications while the issuer carries the application
/// slug).
const ENDPOINTS: &[(&str, &str, bool)] = &[
    (
        "/.well-known/openid-configuration",
        "discovery document",
        true,
    ),
    ("/.well-known/jwks.json", "key set", true),
    ("/oauth/v2/authorize", "authorization endpoint", true),
    ("/oauth/v2/token", "token endpoint", true),
    ("/oauth/v2/keys", "key set", true),
    ("/oidc/v1/userinfo", "userinfo endpoint", true),
    ("/oidc/v1/end_session", "end session endpoint", true),
    (
        "/protocol/openid-connect/auth",
        "authorization endpoint",
        true,
    ),
    ("/protocol/openid-connect/token", "token endpoint", true),
    (
        "/protocol/openid-connect/userinfo",
        "userinfo endpoint",
        true,
    ),
    ("/protocol/openid-connect/certs", "key set", true),
    (
        "/protocol/openid-connect/logout",
        "end session endpoint",
        true,
    ),
    ("/application/o/authorize", "authorization endpoint", false),
    ("/application/o/token", "token endpoint", false),
    ("/application/o/userinfo", "userinfo endpoint", false),
    ("/api/oidc/token", "token endpoint", true),
    ("/api/oidc/userinfo", "userinfo endpoint", true),
    ("/oauth2/authorize", "authorization endpoint", true),
    ("/oauth2/token", "token endpoint", true),
    ("/authorize", "authorization endpoint", true),
    ("/token", "token endpoint", true),
    ("/userinfo", "userinfo endpoint", true),
    ("/jwks", "key set", true),
    ("/keys", "key set", true),
    ("/certs", "key set", true),
];

/// What looks wrong with an issuer URL and, if it can be told, what it probably should be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuerHint {
    /// Plain-language explanation for the admin.
    pub message: String,
    /// The likely issuer (the entered URL without the endpoint path), unverified.
    pub suggested: Option<String>,
}

/// The origin plus the path segments of a URL, or `None` when it is not a plain http(s) URL.
fn split(issuer: &str) -> Option<(String, Vec<String>)> {
    let url = Url::parse(issuer.trim()).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    let origin = url.origin().ascii_serialization();
    let segments = url
        .path_segments()
        .map(|s| {
            s.filter(|p| !p.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Some((origin, segments))
}

fn join(origin: &str, segments: &[String]) -> String {
    if segments.is_empty() {
        origin.to_owned()
    } else {
        format!("{origin}/{}", segments.join("/"))
    }
}

/// Whether `issuer` ends in the path of a known provider endpoint (`/authorize`,
/// `/oauth/v2/authorize`, `/protocol/openid-connect/auth`, `/.well-known/openid-configuration`,
/// `/token`, ...). Generic names such as `/token` can in theory be a real issuer path, so the
/// admin API lets the caller keep the entry on purpose.
#[must_use]
pub fn issuer_hint(issuer: &str) -> Option<IssuerHint> {
    let (origin, segments) = split(issuer)?;
    let path = format!("/{}", segments.join("/"));
    let (suffix, what, strippable) = ENDPOINTS
        .iter()
        .filter(|(suffix, _, _)| path.ends_with(suffix))
        .max_by_key(|(suffix, _, _)| suffix.len())?;
    let keep = segments
        .len()
        .saturating_sub(suffix.split('/').filter(|p| !p.is_empty()).count());
    let suggested = strippable.then(|| join(&origin, &segments[..keep]));
    let mut message = format!(
        "This looks like the provider's {what} ({path}), not its issuer. The issuer is the base \
         address the discovery document is published under: \
         <issuer>/.well-known/openid-configuration."
    );
    match &suggested {
        Some(s) => message.push_str(&format!(" Probably {s} is what you want.")),
        None => message.push_str(
            " Open the application at your provider and copy its \"OpenID configuration issuer\".",
        ),
    }
    Some(IssuerHint { message, suggested })
}

/// A small fixed set of issuers worth trying after discovery for `issuer` failed, all on the
/// same origin: the likely issuer from [`issuer_hint`], the same URL with the trailing slash
/// toggled, and the parent paths down to the origin. Never contains `issuer` itself, never
/// another host, at most [`MAX_CANDIDATES`].
#[must_use]
pub fn candidates(issuer: &str) -> Vec<String> {
    let Some((origin, segments)) = split(issuer) else {
        return Vec::new();
    };
    let given = issuer.trim();
    let mut out: Vec<String> = Vec::new();
    let mut push = |candidate: String| {
        if candidate != given && !out.contains(&candidate) {
            out.push(candidate);
        }
    };
    if let Some(suggested) = issuer_hint(given).and_then(|h| h.suggested) {
        push(suggested);
    }
    if !segments.is_empty() {
        let plain = join(&origin, &segments);
        push(if given.ends_with('/') {
            plain
        } else {
            format!("{plain}/")
        });
    }
    for keep in (0..segments.len()).rev() {
        push(join(&origin, &segments[..keep]));
    }
    out.truncate(MAX_CANDIDATES);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suggested(issuer: &str) -> Option<String> {
        issuer_hint(issuer).and_then(|h| h.suggested)
    }

    #[test]
    fn known_endpoints_get_the_likely_issuer() {
        let cases = [
            (
                "https://auth.example/oauth/v2/authorize",
                "https://auth.example",
            ),
            (
                "https://auth.example/oauth/v2/token",
                "https://auth.example",
            ),
            ("https://auth.example/authorize", "https://auth.example"),
            ("https://auth.example/authorize/", "https://auth.example"),
            ("https://auth.example/token", "https://auth.example"),
            (
                "https://auth.example/.well-known/openid-configuration",
                "https://auth.example",
            ),
            (
                "https://auth.example/realms/home/.well-known/openid-configuration",
                "https://auth.example/realms/home",
            ),
            (
                "https://auth.example/realms/home/protocol/openid-connect/auth",
                "https://auth.example/realms/home",
            ),
            (
                "https://auth.example/auth/realms/home/protocol/openid-connect/token",
                "https://auth.example/auth/realms/home",
            ),
            (
                "https://auth.example:8443/api/oidc/token",
                "https://auth.example:8443",
            ),
            (
                "https://auth.example/application/o/app/.well-known/openid-configuration",
                "https://auth.example/application/o/app",
            ),
        ];
        for (given, want) in cases {
            assert_eq!(suggested(given).as_deref(), Some(want), "{given}");
        }
    }

    #[test]
    fn the_most_specific_endpoint_wins() {
        // `/oauth/v2/authorize` must not be read as just `/authorize`.
        let hint = issuer_hint("https://auth.example/oauth/v2/authorize").expect("hint");
        assert_eq!(hint.suggested.as_deref(), Some("https://auth.example"));
        assert!(hint.message.contains("authorization endpoint"));
    }

    #[test]
    fn authentik_endpoints_explain_instead_of_guessing() {
        let hint = issuer_hint("https://auth.example/application/o/authorize/").expect("hint");
        assert_eq!(hint.suggested, None);
        assert!(hint.message.contains("issuer"));
    }

    #[test]
    fn real_issuers_pass() {
        for issuer in [
            "https://auth.example",
            "https://auth.example/",
            "https://auth.example/realms/home",
            "https://auth.example/application/o/superhet/",
            "https://auth.example/oauth2/default",
            "http://localhost:8080/realms/dev",
            "not a url",
        ] {
            assert_eq!(issuer_hint(issuer), None, "{issuer}");
        }
    }

    #[test]
    fn candidates_stay_on_the_origin_and_are_few() {
        let got = candidates("https://auth.example:8443/oauth/v2/authorize");
        assert_eq!(
            got,
            [
                "https://auth.example:8443",
                "https://auth.example:8443/oauth/v2/authorize/",
                "https://auth.example:8443/oauth/v2",
                "https://auth.example:8443/oauth",
            ]
        );
        assert!(candidates("https://a.example/1/2/3/4/5/6/7/8").len() <= MAX_CANDIDATES);
    }

    #[test]
    fn candidates_toggle_the_trailing_slash_and_skip_the_input() {
        assert_eq!(
            candidates("https://auth.example/application/o/app/"),
            [
                "https://auth.example/application/o/app",
                "https://auth.example/application/o",
                "https://auth.example/application",
                "https://auth.example",
            ]
        );
        assert_eq!(
            candidates("https://auth.example/realms/home"),
            [
                "https://auth.example/realms/home/",
                "https://auth.example/realms",
                "https://auth.example",
            ]
        );
        assert!(candidates("https://auth.example").is_empty());
        assert!(candidates("garbage").is_empty());
    }
}
