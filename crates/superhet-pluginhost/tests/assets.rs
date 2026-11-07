//! Static assets of a plugin's web panel (`[ui]`, ADR-0022): the registry hands out only plain
//! files of a served type below the UI root, and the `asset` control command carries them.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};
use superhet_plugin_protocol::ctl::MediaReplyHeader;
use superhet_plugin_protocol::ui::MAX_ASSET_BYTES;
use superhet_pluginhost::ctl::{Host, handle_bytes};
use superhet_pluginhost::registry::{AssetError, Registry};
use superhet_pluginhost::supervisor::{NullSink, Policy, Supervisor};

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("superhet-assets-{}-{tag}", std::process::id()));
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

const ECHO_ID: &str = "org.superhet.example.echo";

fn echo_source() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/example-echo")
}

/// A registry with the example plugin installed; returns where its UI root lives.
fn installed(dir: &TempDir) -> (Registry, PathBuf) {
    let registry = Registry::open(&dir.0).expect("registry");
    registry.install(&echo_source()).expect("install");
    let ui = dir.0.join(ECHO_ID).join("1.0.0/ui");
    (registry, ui)
}

#[test]
fn the_example_panel_is_served_with_its_content_types() {
    let dir = TempDir::new("serve");
    let (registry, _) = installed(&dir);
    for (path, mime) in [
        ("index.html", "text/html; charset=utf-8"),
        ("panel.js", "text/javascript; charset=utf-8"),
        ("panel.css", "text/css; charset=utf-8"),
    ] {
        let asset = registry.read_asset(ECHO_ID, path).expect(path);
        assert_eq!(asset.mime, mime, "{path}");
        assert!(!asset.bytes.is_empty(), "{path}");
    }
}

#[test]
fn nothing_outside_the_ui_root_leaves_the_host() {
    let dir = TempDir::new("traversal");
    let (registry, ui) = installed(&dir);
    for bad in [
        "../plugin.toml",
        "../../org.superhet.example.echo/1.0.0/plugin.toml",
        "/etc/passwd",
        "ui/../../plugin.toml",
        "a/../index.html",
        "./index.html",
        "..",
        "%2e%2e/plugin.toml",
        "..\\plugin.toml",
        ".hidden",
        "index.html\0.png",
        "",
    ] {
        assert_eq!(
            registry.read_asset(ECHO_ID, bad).map(|_| ()),
            Err(AssetError::Invalid),
            "{bad:?}"
        );
    }
    // The package files next to the root are not part of it, whatever their type.
    assert_eq!(
        registry.read_asset(ECHO_ID, "plugin.toml").map(|_| ()),
        Err(AssetError::NotFound)
    );
    assert_eq!(
        registry.read_asset(ECHO_ID, "bin/example-echo").map(|_| ()),
        Err(AssetError::NotFound)
    );

    // A file of an unserved type below the root stays on the host.
    std::fs::write(ui.join("secret.toml"), "k = 1").expect("write");
    std::fs::write(ui.join("module.wasm"), [0, 97, 115, 109]).expect("write");
    for name in ["secret.toml", "module.wasm"] {
        assert_eq!(
            registry.read_asset(ECHO_ID, name).map(|_| ()),
            Err(AssetError::NotFound),
            "{name}"
        );
    }
}

#[test]
fn links_below_the_root_do_not_lead_out() {
    let dir = TempDir::new("links");
    let (registry, ui) = installed(&dir);
    let outside = TempDir::new("links-outside");
    std::fs::write(outside.0.join("stolen.html"), "<p>secret</p>").expect("write");
    std::fs::create_dir_all(outside.0.join("dir")).expect("dir");
    std::fs::write(outside.0.join("dir/inner.js"), "1").expect("write");

    // `install` refuses links, so plant them in the installed copy as an attacker with write
    // access to the plugin directory (or a future installer bug) would.
    std::os::unix::fs::symlink(outside.0.join("stolen.html"), ui.join("link.html")).expect("ln");
    std::os::unix::fs::symlink(&outside.0, ui.join("linkdir")).expect("ln");
    std::os::unix::fs::symlink("../plugin.toml", ui.join("inner.json")).expect("ln");
    std::os::unix::fs::symlink("index.html", ui.join("alias.html")).expect("ln");
    for path in [
        "link.html",
        "linkdir/stolen.html",
        "linkdir/dir/inner.js",
        "inner.json",
        "alias.html",
    ] {
        assert_eq!(
            registry.read_asset(ECHO_ID, path).map(|_| ()),
            Err(AssetError::NotFound),
            "{path}"
        );
    }
    // The plain file is still served.
    assert!(registry.read_asset(ECHO_ID, "index.html").is_ok());
}

#[test]
fn unknown_plugins_directories_and_oversized_files_are_refused() {
    let dir = TempDir::new("limits");
    let (registry, ui) = installed(&dir);
    assert_eq!(
        registry
            .read_asset("org.superhet.nope.nope", "index.html")
            .map(|_| ()),
        Err(AssetError::NotFound)
    );
    std::fs::create_dir_all(ui.join("sub.js")).expect("dir");
    assert_eq!(
        registry.read_asset(ECHO_ID, "sub.js").map(|_| ()),
        Err(AssetError::NotFound)
    );
    std::fs::write(ui.join("big.js"), vec![b'a'; MAX_ASSET_BYTES + 1]).expect("write");
    assert_eq!(
        registry.read_asset(ECHO_ID, "big.js").map(|_| ()),
        Err(AssetError::TooLarge)
    );
    std::fs::write(ui.join("max.js"), vec![b'a'; MAX_ASSET_BYTES]).expect("write");
    assert!(registry.read_asset(ECHO_ID, "max.js").is_ok());
}

#[test]
fn a_plugin_without_a_panel_serves_nothing() {
    let dir = TempDir::new("nopanel");
    let registry = Registry::open(&dir.0).expect("registry");
    let source = TempDir::new("nopanel-src");
    std::fs::create_dir_all(source.0.join("ui")).expect("dir");
    std::fs::write(source.0.join("ui/index.html"), "x").expect("write");
    std::fs::write(source.0.join("a.json"), "{}").expect("write");
    let manifest = std::fs::read_to_string(echo_source().join("plugin.toml")).expect("manifest");
    let cut = manifest.find("# Optional web panel").expect("ui section");
    std::fs::write(source.0.join("plugin.toml"), &manifest[..cut]).expect("write");
    std::fs::create_dir_all(source.0.join("schema")).expect("dir");
    std::fs::copy(
        echo_source().join("schema/events.json"),
        source.0.join("schema/events.json"),
    )
    .expect("copy");
    std::fs::create_dir_all(source.0.join("bin")).expect("dir");
    std::fs::write(source.0.join("bin/example-echo"), "").expect("write");
    registry.install(&source.0).expect("install");
    assert_eq!(
        registry.read_asset(ECHO_ID, "index.html").map(|_| ()),
        Err(AssetError::NotFound)
    );
}

fn host(dir: &Path) -> Host {
    Host {
        registry: Arc::new(Registry::open(dir).expect("registry")),
        supervisor: Supervisor::new(Policy::default(), Arc::new(NullSink)),
        store: Default::default(),
    }
}

async fn asset(host: &Host, body: Value) -> Vec<u8> {
    handle_bytes(host, "asset", body.to_string().as_bytes()).await
}

#[tokio::test]
async fn the_asset_command_carries_bytes_and_error_codes() {
    let dir = TempDir::new("ctl");
    let host = host(&dir.0);
    host.registry.install(&echo_source()).expect("install");

    let ok = asset(
        &host,
        json!({"v": 1, "plugin_id": ECHO_ID, "path": "index.html"}),
    )
    .await;
    let (header, bytes) = MediaReplyHeader::decode(&ok).expect("success reply");
    assert_eq!(header.key, "index.html");
    assert_eq!(header.mime, "text/html; charset=utf-8");
    assert!(bytes.starts_with(b"<!doctype html>"));

    for (body, code) in [
        (
            json!({"v": 1, "plugin_id": ECHO_ID, "path": "../plugin.toml"}),
            "invalid",
        ),
        (
            json!({"v": 1, "plugin_id": ECHO_ID, "path": "nope.js"}),
            "not_found",
        ),
        (
            json!({"v": 1, "plugin_id": "org.superhet.nope.nope", "path": "a.js"}),
            "not_found",
        ),
        (
            json!({"v": 2, "plugin_id": ECHO_ID, "path": "index.html"}),
            "invalid",
        ),
        (json!({"v": 1, "plugin_id": ECHO_ID}), "invalid"),
    ] {
        let reply = asset(&host, body.clone()).await;
        assert!(MediaReplyHeader::decode(&reply).is_none(), "{body}");
        let value: Value = serde_json::from_slice(&reply).expect("json error");
        assert_eq!(
            (value["ok"].clone(), value["code"].clone()),
            (json!(false), json!(code)),
            "{body}"
        );
    }
}
