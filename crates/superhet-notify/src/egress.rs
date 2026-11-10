//! The HTTP client every outbound notification goes through: SSRF-guarded (see [`crate::ssrf`]),
//! no system proxy, bounded in time and redirects, and error texts that never contain the URL
//! (webhook URLs routinely carry tokens).

use std::error::Error as _;
use std::time::Duration;

use reqwest::{Client, Method, StatusCode, header::HeaderMap, redirect::Policy};
use url::Url;

use crate::ssrf::{self, BlockedHost, GuardedResolver};

/// Redirects followed by hand, each checked like the first request.
const MAX_REDIRECTS: usize = 3;
/// How long connecting may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(8);

/// A failed delivery attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendError {
    /// Text for the delivery log; never contains URLs, keys or tokens.
    pub message: String,
    /// Whether trying again later can help.
    pub retryable: bool,
    /// The receiver says the destination no longer exists (push subscription expired).
    pub gone: bool,
}

impl SendError {
    /// A failure worth retrying.
    #[must_use]
    pub fn transient(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
            gone: false,
        }
    }

    /// A failure that will not get better by itself.
    #[must_use]
    pub fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
            gone: false,
        }
    }
}

impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SendError {}

/// What a plain HTTP delivery returned.
#[must_use]
pub fn status_error(status: StatusCode) -> Option<SendError> {
    if status.is_success() {
        return None;
    }
    let text = format!(
        "HTTP {} {}",
        status.as_u16(),
        status.canonical_reason().unwrap_or("")
    )
    .trim()
    .to_owned();
    let retryable = status.is_server_error()
        || matches!(
            status,
            StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_EARLY | StatusCode::TOO_MANY_REQUESTS
        );
    Some(SendError {
        message: text,
        retryable,
        gone: false,
    })
}

/// One outbound request.
#[derive(Debug)]
pub struct Request<'a> {
    /// Method (POST for webhooks and push).
    pub method: Method,
    /// Destination.
    pub url: &'a Url,
    /// Headers.
    pub headers: HeaderMap,
    /// Body.
    pub body: Vec<u8>,
    /// Overall time limit of one hop.
    pub timeout: Duration,
    /// The target allows private addresses.
    pub allow_private: bool,
}

/// Two clients, one per policy (the resolver of each filters differently).
#[derive(Debug, Clone)]
pub struct Egress {
    strict: Client,
    relaxed: Client,
}

fn client(allow_private: bool) -> Result<Client, String> {
    Client::builder()
        .dns_resolver(GuardedResolver::new(allow_private))
        .redirect(Policy::none())
        .no_proxy()
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(concat!("Superhet/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.without_url().to_string())
}

impl Egress {
    /// Build the clients.
    ///
    /// # Errors
    /// A message if the TLS stack cannot be set up.
    pub fn new() -> Result<Self, String> {
        // reqwest is built without a bundled crypto provider; the workspace uses ring. Already
        // installed is fine.
        let _ = rustls::crypto::ring::default_provider().install_default();
        Ok(Self {
            strict: client(false)?,
            relaxed: client(true)?,
        })
    }

    /// Send and return the final status. 307/308 redirects are followed (at most three, every
    /// hop checked); any other redirect is an error. Other statuses are the caller's to judge.
    ///
    /// # Errors
    /// A [`SendError`] when the request is refused by the guard or fails to complete.
    pub async fn send(&self, req: &Request<'_>) -> Result<StatusCode, SendError> {
        let client = if req.allow_private {
            &self.relaxed
        } else {
            &self.strict
        };
        let mut url = req.url.clone();
        for _ in 0..=MAX_REDIRECTS {
            ssrf::check_url(&url, req.allow_private).map_err(SendError::permanent)?;
            let response = client
                .request(req.method.clone(), url.clone())
                .headers(req.headers.clone())
                .body(req.body.clone())
                .timeout(req.timeout)
                .send()
                .await
                .map_err(describe)?;
            let status = response.status();
            if !status.is_redirection() {
                return Ok(status);
            }
            if !matches!(status.as_u16(), 307 | 308) {
                return Err(SendError::permanent(format!(
                    "HTTP {} redirect not followed (only 307/308 are)",
                    status.as_u16()
                )));
            }
            let next = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|location| url.join(location).ok())
                .ok_or_else(|| SendError::permanent("redirect without a usable Location"))?;
            url = next;
        }
        Err(SendError::permanent("too many redirects"))
    }
}

/// A log-safe description of a transport error.
fn describe(err: reqwest::Error) -> SendError {
    let mut source = err.source();
    while let Some(inner) = source {
        if let Some(blocked) = inner.downcast_ref::<BlockedHost>() {
            return SendError::permanent(blocked.0);
        }
        source = inner.source();
    }
    let err = err.without_url();
    let text = if err.is_timeout() {
        "timed out".to_owned()
    } else if err.is_connect() {
        let mut detail = String::new();
        let mut source = err.source();
        while let Some(inner) = source {
            detail = inner.to_string();
            source = inner.source();
        }
        format!(
            "connection failed: {}",
            detail.chars().take(120).collect::<String>()
        )
    } else if err.is_request() || err.is_builder() {
        "request could not be sent".to_owned()
    } else {
        "transfer failed".to_owned()
    };
    SendError::transient(text)
}
