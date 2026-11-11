//! E-mail delivery through the SMTP server named by `SUPERHET_SMTP_*` (worker environment).

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use lettre::message::{Mailbox, header::ContentType};
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use crate::egress::SendError;

/// How the connection to the SMTP server is secured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Security {
    /// Plain connection upgraded with STARTTLS (port 587); required, no fallback to plain text.
    StartTls,
    /// TLS from the first byte (port 465).
    Tls,
    /// No encryption (a local relay only).
    None,
}

/// SMTP settings.
#[derive(Clone)]
pub struct SmtpConfig {
    /// Server name.
    pub host: String,
    /// Port.
    pub port: u16,
    /// Account name, if the server requires login.
    pub user: Option<String>,
    /// Account password. Secret.
    pub password: Option<String>,
    /// Sender address (`Superhet <alerts@example.com>`).
    pub from: Mailbox,
    /// Transport security.
    pub security: Security,
}

impl fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("security", &self.security)
            .finish_non_exhaustive()
    }
}

impl SmtpConfig {
    /// Read `SUPERHET_SMTP_HOST`, `_PORT`, `_USER`, `_PASSWORD`, `_FROM`, `_TLS`
    /// (`starttls` default, `tls`, `none`) through `get`. `None` when no host is set.
    ///
    /// # Errors
    /// A message for an incomplete or invalid combination.
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Result<Option<Self>, String> {
        let var = |name: &str| {
            get(name)
                .map(|v| v.trim().to_owned())
                .filter(|v| !v.is_empty())
        };
        let Some(host) = var("SUPERHET_SMTP_HOST") else {
            return Ok(None);
        };
        let security = match var("SUPERHET_SMTP_TLS")
            .as_deref()
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            None | Some("starttls") => Security::StartTls,
            Some("tls") => Security::Tls,
            Some("none") => Security::None,
            Some(_) => return Err("SUPERHET_SMTP_TLS must be starttls, tls or none".to_owned()),
        };
        let port = match var("SUPERHET_SMTP_PORT") {
            Some(p) => p
                .parse::<u16>()
                .ok()
                .filter(|p| *p > 0)
                .ok_or("SUPERHET_SMTP_PORT must be a port number")?,
            None => match security {
                Security::StartTls => 587,
                Security::Tls => 465,
                Security::None => 25,
            },
        };
        let from = var("SUPERHET_SMTP_FROM")
            .ok_or("SUPERHET_SMTP_FROM is required with SUPERHET_SMTP_HOST")?;
        let from = Mailbox::from_str(&from)
            .map_err(|_| "SUPERHET_SMTP_FROM is not a valid address".to_owned())?;
        let (user, password) = (
            var("SUPERHET_SMTP_USER"),
            get("SUPERHET_SMTP_PASSWORD").filter(|p| !p.is_empty()),
        );
        if user.is_some() != password.is_some() {
            return Err("SUPERHET_SMTP_USER and SUPERHET_SMTP_PASSWORD go together".to_owned());
        }
        Ok(Some(Self {
            host,
            port,
            user,
            password,
            from,
            security,
        }))
    }
}

const TIMEOUT: Duration = Duration::from_secs(20);

fn line(text: &str, max: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Send one mail.
///
/// # Errors
/// A [`SendError`]; the text never contains the password.
pub async fn send(
    cfg: &SmtpConfig,
    to: &[String],
    subject: &str,
    body: &str,
) -> Result<(), SendError> {
    let mut builder = Message::builder()
        .from(cfg.from.clone())
        .subject(line(subject, 200))
        .header(ContentType::TEXT_PLAIN);
    for addr in to {
        let mailbox = Mailbox::from_str(addr)
            .map_err(|_| SendError::permanent("recipient address is not valid"))?;
        builder = builder.to(mailbox);
    }
    let mail = builder
        .body(body.to_owned())
        .map_err(|_| SendError::permanent("the message could not be built"))?;
    let relay = match cfg.security {
        Security::StartTls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&cfg.host),
        Security::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&cfg.host),
        Security::None => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
            &cfg.host,
        )),
    }
    .map_err(|_| SendError::permanent("SMTP host name is not valid"))?;
    let mut relay = relay.port(cfg.port).timeout(Some(TIMEOUT));
    if let (Some(user), Some(password)) = (&cfg.user, &cfg.password) {
        relay = relay.credentials(Credentials::new(user.clone(), password.clone()));
    }
    match relay.build().send(mail).await {
        Ok(_) => Ok(()),
        Err(err) if err.is_permanent() => Err(SendError::permanent(format!(
            "SMTP rejected the message: {}",
            line(&err.to_string(), 160)
        ))),
        Err(err) => Err(SendError::transient(format!(
            "SMTP failed: {}",
            line(&err.to_string(), 160)
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn smtp_settings_come_from_the_environment() {
        assert!(SmtpConfig::from_env(env(&[])).expect("ok").is_none());
        let cfg = SmtpConfig::from_env(env(&[
            ("SUPERHET_SMTP_HOST", "mail.example.com"),
            ("SUPERHET_SMTP_FROM", "Superhet <alerts@example.com>"),
            ("SUPERHET_SMTP_USER", "u"),
            ("SUPERHET_SMTP_PASSWORD", "hunter2"),
        ]))
        .expect("ok")
        .expect("configured");
        assert_eq!((cfg.port, cfg.security), (587, Security::StartTls));
        let shown = format!("{cfg:?}");
        assert!(!shown.contains("hunter2"), "{shown}");
        let tls = SmtpConfig::from_env(env(&[
            ("SUPERHET_SMTP_HOST", "m"),
            ("SUPERHET_SMTP_FROM", "a@example.com"),
            ("SUPERHET_SMTP_TLS", "tls"),
        ]))
        .expect("ok")
        .expect("configured");
        assert_eq!(tls.port, 465);
        for bad in [
            vec![("SUPERHET_SMTP_HOST", "m")],
            vec![("SUPERHET_SMTP_HOST", "m"), ("SUPERHET_SMTP_FROM", "nope")],
            vec![
                ("SUPERHET_SMTP_HOST", "m"),
                ("SUPERHET_SMTP_FROM", "a@example.com"),
                ("SUPERHET_SMTP_USER", "u"),
            ],
            vec![
                ("SUPERHET_SMTP_HOST", "m"),
                ("SUPERHET_SMTP_FROM", "a@example.com"),
                ("SUPERHET_SMTP_TLS", "weird"),
            ],
            vec![
                ("SUPERHET_SMTP_HOST", "m"),
                ("SUPERHET_SMTP_FROM", "a@example.com"),
                ("SUPERHET_SMTP_PORT", "99999"),
            ],
        ] {
            assert!(SmtpConfig::from_env(env(&bad)).is_err(), "{bad:?}");
        }
    }

    /// A tiny SMTP server that accepts one message without TLS and returns the DATA section.
    async fn mock_smtp(reject: bool) -> (u16, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let (read, mut write) = stream.into_split();
            let mut read = BufReader::new(read);
            write.write_all(b"220 mock ESMTP\r\n").await.expect("w");
            let mut data = String::new();
            let mut in_data = false;
            let mut line = String::new();
            loop {
                line.clear();
                if read.read_line(&mut line).await.expect("r") == 0 {
                    break;
                }
                if in_data {
                    if line == ".\r\n" {
                        in_data = false;
                        write.write_all(b"250 queued\r\n").await.expect("w");
                    } else {
                        data.push_str(&line);
                    }
                    continue;
                }
                let upper = line.to_ascii_uppercase();
                let reply: &[u8] = if upper.starts_with("EHLO") {
                    b"250-mock\r\n250 8BITMIME\r\n"
                } else if upper.starts_with("RCPT") && reject {
                    b"550 no such user\r\n"
                } else if upper.starts_with("DATA") {
                    in_data = true;
                    b"354 go ahead\r\n"
                } else if upper.starts_with("QUIT") {
                    write.write_all(b"221 bye\r\n").await.expect("w");
                    break;
                } else {
                    b"250 ok\r\n"
                };
                write.write_all(reply).await.expect("w");
            }
            data
        });
        (port, task)
    }

    fn cfg(port: u16) -> SmtpConfig {
        SmtpConfig {
            host: "127.0.0.1".to_owned(),
            port,
            user: None,
            password: None,
            from: Mailbox::from_str("Superhet <alerts@example.com>").expect("from"),
            security: Security::None,
        }
    }

    #[tokio::test]
    async fn mail_reaches_the_server_with_subject_and_body() {
        let (port, server) = mock_smtp(false).await;
        send(
            &cfg(port),
            &["ops@example.com".to_owned()],
            "Fire pager\r\nBcc: evil@example.com",
            "FIRE at Main Street",
        )
        .await
        .expect("sent");
        let data = server.await.expect("server");
        assert!(data.contains("To: ops@example.com"), "{data}");
        assert!(
            data.contains("Subject: Fire pager Bcc: evil@example.com")
                || data.contains("Subject: Fire pagerBcc"),
            "{data}"
        );
        assert!(!data.contains("\r\nBcc:"), "no header injection: {data}");
        assert!(data.contains("FIRE at Main Street"), "{data}");
    }

    #[tokio::test]
    async fn a_refused_recipient_is_a_permanent_failure() {
        let (port, server) = mock_smtp(true).await;
        let err = send(&cfg(port), &["ops@example.com".to_owned()], "s", "b")
            .await
            .expect_err("rejected");
        assert!(!err.retryable, "{err}");
        let _ = server.await;
    }

    #[tokio::test]
    async fn an_unreachable_server_is_transient() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);
        let err = send(&cfg(port), &["ops@example.com".to_owned()], "s", "b")
            .await
            .expect_err("refused");
        assert!(err.retryable, "{err}");
    }
}
