//! Helpers shared by the integration tests that run the real `superhet-api` binary against a
//! real PostgreSQL (`DATABASE_URL`, an admin-capable URL; each test gets its own database).
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const ADMIN_USER: &str = "admin";
pub const ADMIN_PASSWORD: &str = "correct horse battery staple";
pub const SETUP_TOKEN: &str = "integration-test-setup-token";

/// `DATABASE_URL`, if the environment provides a PostgreSQL for tests.
pub fn admin_database_url() -> Option<String> {
    std::env::var("DATABASE_URL").ok().filter(|u| !u.is_empty())
}

/// A throwaway database, dropped (with `FORCE`) when the value goes away.
pub struct TestDb {
    admin_url: String,
    pub name: String,
    pub url: String,
}

impl TestDb {
    pub async fn create(tag: &str) -> Option<Self> {
        let admin_url = admin_database_url()?;
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let name = format!(
            "sh_api_{tag}_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let admin = superhet_db::connect(&admin_url)
            .await
            .expect("connect admin");
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&admin)
            .await
            .expect("create database");
        let (base, _) = admin_url.rsplit_once('/').expect("url with database");
        let url = format!("{base}/{name}");
        Some(Self {
            admin_url,
            name,
            url,
        })
    }

    pub async fn pool(&self) -> superhet_db::PgPool {
        superhet_db::connect(&self.url)
            .await
            .expect("connect test db")
    }
}

impl Drop for TestDb {
    fn drop(&mut self) {
        let (admin_url, name) = (self.admin_url.clone(), self.name.clone());
        // Own thread + runtime: Drop may run inside or outside a Tokio runtime.
        let _ = std::thread::spawn(move || {
            let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            else {
                return;
            };
            rt.block_on(async {
                if let Ok(admin) = superhet_db::connect(&admin_url).await {
                    let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
                        "DROP DATABASE IF EXISTS {name} WITH (FORCE)"
                    )))
                    .execute(&admin)
                    .await;
                }
            });
        })
        .join();
    }
}

/// A child process that is killed on drop.
pub struct Proc(pub Child);

impl Drop for Proc {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A TCP port that no other test (in this or any concurrently running test binary) hands out.
///
/// Asking the OS for port 0 and releasing it again is racy: the service under test needs a while
/// to start (migrations first) and binds late, so two tests can be given the same port and one
/// would talk to the other's server. Every candidate is therefore claimed with an advisory lock
/// file that is held until the test process exits.
pub fn free_port() -> u16 {
    static CLAIMED: std::sync::Mutex<Vec<std::fs::File>> = std::sync::Mutex::new(Vec::new());
    loop {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .expect("free port")
            .port();
        let path = std::env::temp_dir().join(format!("superhet-test-port-{port}.lock"));
        let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
        else {
            continue;
        };
        if file.try_lock().is_ok() {
            if let Ok(mut claimed) = CLAIMED.lock() {
                claimed.push(file);
            }
            return port;
        }
    }
}

/// A response from the raw HTTP client below.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    /// Header names lower-cased, in order.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.body).unwrap_or(serde_json::Value::Null)
    }

    /// The full `Set-Cookie` header, if any.
    pub fn set_cookie(&self) -> Option<&str> {
        self.header("set-cookie")
    }

    /// `name=value` of the session cookie to send back (empty when it is being cleared).
    pub fn session_cookie(&self) -> Option<String> {
        let first = self.set_cookie()?.split(';').next()?;
        Some(first.to_owned())
    }
}

/// Minimal HTTP/1.0 client (one connection per request, blocking).
pub fn http(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Reply {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("timeout");
    let mut request = format!(
        "{method} {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("\r\n");
    request.push_str(body);
    stream.write_all(request.as_bytes()).expect("write");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read");
    let (head, body) = response.split_once("\r\n\r\n").unwrap_or((&response, ""));
    let mut lines = head.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .expect("status line");
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    Reply {
        status,
        headers,
        body: body.to_owned(),
    }
}

/// `http` on the blocking pool, for use from async tests.
pub async fn http_async(
    port: u16,
    method: &'static str,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
) -> Reply {
    tokio::task::spawn_blocking(move || {
        let headers: Vec<(&str, &str)> = headers
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        http(port, method, &path, &headers, &body)
    })
    .await
    .expect("join")
}

pub fn readyz_status(port: u16) -> Option<u16> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok()?;
    stream
        .write_all(b"GET /readyz HTTP/1.0\r\nHost: localhost\r\n\r\n")
        .ok()?;
    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;
    response.split_whitespace().nth(1)?.parse().ok()
}

pub async fn wait_ready(port: u16, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while readyz_status(port) != Some(200) {
        assert!(Instant::now() < deadline, "{what} /readyz never became 200");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Spawn the API under test with the database and the fixed setup token; callers add more
/// environment through `configure`.
pub fn spawn_api(db_url: &str, configure: impl FnOnce(&mut Command)) -> (Proc, u16) {
    let port = free_port();
    let mut command = Command::new(env!("CARGO_BIN_EXE_superhet-api"));
    command
        .env("SUPERHET_LISTEN", format!("127.0.0.1:{port}"))
        .env("SUPERHET_DATABASE_URL", db_url)
        .env("SUPERHET_SETUP_TOKEN", SETUP_TOKEN)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure(&mut command);
    (Proc(command.spawn().expect("spawn api")), port)
}

/// Create the admin account through the API and return the `name=value` session cookie.
pub fn setup_admin(port: u16) -> String {
    let body = serde_json::json!({
        "username": ADMIN_USER,
        "password": ADMIN_PASSWORD,
        "setup_token": SETUP_TOKEN,
    })
    .to_string();
    let reply = http(
        port,
        "POST",
        "/api/v1/auth/setup",
        &[("Content-Type", "application/json")],
        &body,
    );
    assert_eq!(reply.status, 204, "setup failed: {reply:?}");
    reply.session_cookie().expect("session cookie after setup")
}

/// Open a WebSocket carrying the session cookie.
pub async fn ws_connect_with_cookie(
    url: &str,
    cookie: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::Error,
> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = url.into_client_request()?;
    request
        .headers_mut()
        .insert("Cookie", cookie.parse().expect("cookie header value"));
    tokio_tungstenite::connect_async(request)
        .await
        .map(|(ws, _)| ws)
}
