//! Control API: install from a local path, list, start, get, stop, errors. Transport free
//! (`ctl::handle`); the bus wiring is covered by `nats_live.rs`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use superhet_pluginhost::ctl::{Host, handle};
use superhet_pluginhost::registry::Registry;
use superhet_pluginhost::supervisor::{NullSink, Policy, Supervisor};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("superhet-ctl-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn echo_source() -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins/example-echo")
        .display()
        .to_string()
}

fn host(dir: &Path) -> Host {
    Host {
        registry: Arc::new(Registry::open(dir).expect("registry")),
        supervisor: Supervisor::new(Policy::default(), Arc::new(NullSink)),
    }
}

async fn call(host: &Host, cmd: &str, body: Value) -> Value {
    handle(host, cmd, body.to_string().as_bytes()).await
}

#[tokio::test]
async fn install_list_start_get_stop() {
    let dir = TempDir::new("flow");
    let host = host(&dir.0);

    let empty = call(&host, "list", json!({"v": 1})).await;
    assert_eq!(empty["ok"], true);
    assert_eq!(empty["plugins"], json!([]));

    let installed = call(&host, "install", json!({"v": 1, "path": echo_source()})).await;
    assert_eq!(installed["ok"], true, "{installed}");
    assert_eq!(
        installed["plugin"]["manifest"]["plugin"]["id"],
        "org.superhet.example.echo"
    );
    // Copied to <dir>/<id>/<version>, executable bit preserved.
    let copy = dir.0.join("org.superhet.example.echo/1.0.0");
    assert!(copy.join("plugin.toml").is_file());
    let mode = std::os::unix::fs::PermissionsExt::mode(
        &std::fs::metadata(copy.join("bin/example-echo"))
            .expect("meta")
            .permissions(),
    );
    assert_ne!(mode & 0o111, 0);

    let again = call(&host, "install", json!({"v": 1, "path": echo_source()})).await;
    assert_eq!(
        (again["ok"].clone(), again["code"].clone()),
        (json!(false), json!("conflict"))
    );

    let listed = call(&host, "list", json!({"v": 1})).await;
    assert_eq!(listed["plugins"].as_array().map(Vec::len), Some(1));

    let started = call(
        &host,
        "start",
        json!({"v": 1, "plugin_id": "org.superhet.example.echo", "device": "dev0", "freq_hz": 1000}),
    )
    .await;
    assert_eq!(started["ok"], true, "{started}");
    let id = started["instance"]["id"].as_str().expect("id").to_owned();
    assert_eq!(started["instance"]["device"], "dev0");

    let all = call(&host, "instances", json!({"v": 1})).await;
    assert_eq!(all["instances"].as_array().map(Vec::len), Some(1));
    let one = call(&host, "get", json!({"v": 1, "id": id})).await;
    assert_eq!(one["instance"]["plugin_id"], "org.superhet.example.echo");

    let stopped = call(&host, "stop", json!({"v": 1, "id": id})).await;
    assert_eq!(stopped["instance"]["state"], "stopped");
    let gone = call(&host, "get", json!({"v": 1, "id": id})).await;
    assert_eq!(gone["code"], "not_found");

    // A fresh registry over the same directory finds the installed plugin again.
    let reopened = Registry::open(&dir.0).expect("reopen");
    assert!(reopened.get("org.superhet.example.echo").is_some());
}

#[tokio::test]
async fn bad_requests_get_error_replies() {
    let dir = TempDir::new("errors");
    let host = host(&dir.0);
    for (cmd, body, want) in [
        ("start", json!({"plugin_id": "x.y.z"}), "invalid"), // no "v"
        (
            "start",
            json!({"v": 1, "plugin_id": "org.none.here"}),
            "not_found",
        ),
        (
            "start",
            json!({"v": 1, "plugin_id": "x", "bogus": 1}),
            "invalid",
        ),
        (
            "install",
            json!({"v": 1, "path": "/nonexistent/dir"}),
            "invalid",
        ),
        ("stop", json!({"v": 1, "id": "i-none"}), "not_found"),
        ("restart", json!({"v": 1, "id": "i-none"}), "not_found"),
        ("frobnicate", json!({"v": 1}), "not_found"),
    ] {
        let reply = call(&host, cmd, body).await;
        assert_eq!(reply["ok"], false, "{cmd}: {reply}");
        assert_eq!(reply["code"], want, "{cmd}: {reply}");
        assert_eq!(reply["v"], 1);
    }
    let not_json = handle(&host, "start", b"{{").await;
    assert_eq!(not_json["code"], "invalid");
}

#[tokio::test]
async fn install_refuses_symlinks_and_bad_manifests() {
    let dir = TempDir::new("refuse");
    let host = host(&dir.0.join("installed"));

    let src = dir.0.join("evil");
    std::fs::create_dir_all(&src).expect("mkdir");
    let manifest =
        std::fs::read_to_string(Path::new(&echo_source()).join("plugin.toml")).expect("manifest");
    std::fs::write(
        src.join("plugin.toml"),
        manifest.replace("bin/example-echo", "../escape"),
    )
    .expect("write");
    let reply = call(
        &host,
        "install",
        json!({"v": 1, "path": src.display().to_string()}),
    )
    .await;
    assert_eq!(reply["code"], "invalid");
    assert!(
        reply["error"].as_str().unwrap_or("").contains("inside"),
        "{reply}"
    );

    // Valid manifest, but a symlink inside the tree.
    std::fs::write(src.join("plugin.toml"), manifest).expect("write");
    std::fs::create_dir_all(src.join("schema")).expect("mkdir");
    std::fs::copy(
        Path::new(&echo_source()).join("schema/events.json"),
        src.join("schema/events.json"),
    )
    .expect("copy");
    std::os::unix::fs::symlink("/etc/passwd", src.join("link")).expect("symlink");
    let reply = call(
        &host,
        "install",
        json!({"v": 1, "path": src.display().to_string()}),
    )
    .await;
    assert_eq!(reply["code"], "invalid");
    assert!(
        reply["error"]
            .as_str()
            .unwrap_or("")
            .contains("symbolic link"),
        "{reply}"
    );
    // Nothing half-installed is left behind.
    let leftovers: Vec<_> = std::fs::read_dir(dir.0.join("installed"))
        .expect("dir")
        .flatten()
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}
