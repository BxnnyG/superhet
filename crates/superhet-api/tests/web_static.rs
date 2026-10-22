//! The real `superhet-api` binary serving a built web UI from `SUPERHET_WEB_DIR`. Needs no
//! database or bus: the static files are public and the API paths still answer with JSON.

mod common;

use std::process::{Command, Stdio};

use common::{Proc, Reply, free_port, http};

const INDEX: &str = "<!doctype html><title>Superhet</title><div id=root></div>";

struct Site {
    _dir: std::path::PathBuf,
    _proc: Proc,
    port: u16,
}

impl Site {
    fn start(with_ui: bool) -> Self {
        let port = free_port();
        let dir = std::env::temp_dir().join(format!("superhet-web-{}-{port}", std::process::id()));
        std::fs::create_dir_all(dir.join("assets")).expect("mkdir");
        if with_ui {
            std::fs::write(dir.join("index.html"), INDEX).expect("index");
            std::fs::write(dir.join("assets/x.js"), "console.log(1)").expect("asset");
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_superhet-api"));
        command
            .env("SUPERHET_LISTEN", format!("127.0.0.1:{port}"))
            .env("SUPERHET_WEB_DIR", &dir)
            .env_remove("SUPERHET_DATABASE_URL")
            .env_remove("SUPERHET_NATS_URL")
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let proc = Proc(command.spawn().expect("spawn api"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while common::readyz_status(port) != Some(200) {
            assert!(std::time::Instant::now() < deadline, "api never ready");
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        Self {
            _dir: dir,
            _proc: proc,
            port,
        }
    }

    fn get(&self, path: &str) -> Reply {
        http(self.port, "GET", path, &[], "")
    }
}

impl Drop for Site {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self._dir);
    }
}

#[test]
fn serves_index_and_spa_routes_without_a_session() {
    let site = Site::start(true);
    for path in ["/", "/index.html", "/some/route", "/settings/devices"] {
        let reply = site.get(path);
        assert_eq!(reply.status, 200, "{path}");
        assert_eq!(reply.body, INDEX, "{path}");
        assert_eq!(reply.header("cache-control"), Some("no-cache"), "{path}");
        assert_eq!(reply.header("x-content-type-options"), Some("nosniff"));
        assert_eq!(reply.header("x-frame-options"), Some("DENY"));
        assert_eq!(reply.header("referrer-policy"), Some("same-origin"));
        let csp = reply.header("content-security-policy").expect("csp");
        assert!(csp.contains("frame-ancestors 'none'"), "{csp}");
    }
}

#[test]
fn hashed_assets_are_immutable_and_missing_files_are_404() {
    let site = Site::start(true);
    let reply = site.get("/assets/x.js");
    assert_eq!(reply.status, 200);
    assert_eq!(reply.body, "console.log(1)");
    assert_eq!(
        reply.header("cache-control"),
        Some("public, max-age=31536000, immutable")
    );
    assert!(
        reply
            .header("content-type")
            .is_some_and(|t| t.contains("javascript"))
    );
    for path in ["/assets/missing.js", "/favicon.ico"] {
        let reply = site.get(path);
        assert_eq!(reply.status, 404, "{path}");
        assert!(!reply.body.contains("<title>"), "{path}");
    }
}

#[test]
fn api_paths_never_fall_back_to_the_ui() {
    let site = Site::start(true);
    let reply = site.get("/api/v1/nope");
    assert_eq!(reply.status, 401);
    assert_eq!(reply.body, r#"{"error":"unauthenticated"}"#);
    let reply = site.get("/api/v1/auth/status");
    assert_eq!(reply.status, 503);
    assert_eq!(reply.body, r#"{"error":"unavailable"}"#);
    assert_eq!(site.get("/healthz").body, "ok");
}

#[test]
fn missing_ui_answers_a_hint_but_keeps_the_api() {
    let site = Site::start(false);
    let reply = site.get("/");
    assert_eq!(reply.status, 404);
    assert!(reply.body.contains("SUPERHET_WEB_DIR"), "{}", reply.body);
    assert_eq!(site.get("/api/v1/nope").status, 401);
}
