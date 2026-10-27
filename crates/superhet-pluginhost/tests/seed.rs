//! Plugins shipped with the image are installed once (`Registry::seed_bundled`).

use std::path::PathBuf;

use superhet_pluginhost::registry::Registry;

fn temp(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("superhet-seed-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn bundled() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugins")
}

#[test]
fn first_party_plugins_are_installed_once() {
    let dir = temp("once");
    let registry = Registry::open(&dir).expect("open");
    let installed = registry.seed_bundled(&bundled()).expect("seed");
    for id in [
        "org.superhet.decoder.pocsag",
        "org.superhet.decoder.aprs",
        "org.superhet.decoder.rtl433",
    ] {
        assert!(registry.get(id).is_some(), "{id} missing");
        assert!(
            installed.iter().any(|k| k.starts_with(id)),
            "{id} not reported"
        );
    }

    // A second start installs nothing.
    let registry = Registry::open(&dir).expect("reopen");
    assert!(
        registry
            .seed_bundled(&bundled())
            .expect("seed again")
            .is_empty()
    );

    // An admin removes a plugin; it stays removed over restarts.
    registry
        .remove("org.superhet.decoder.rtl433")
        .expect("remove");
    let registry = Registry::open(&dir).expect("reopen");
    assert!(registry.seed_bundled(&bundled()).expect("seed").is_empty());
    assert!(registry.get("org.superhet.decoder.rtl433").is_none());
    assert!(registry.get("org.superhet.decoder.aprs").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_bundled_dir_is_fine() {
    let dir = temp("missing");
    let registry = Registry::open(&dir).expect("open");
    let none = registry
        .seed_bundled(&dir.join("does-not-exist"))
        .expect("seed");
    assert!(none.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
