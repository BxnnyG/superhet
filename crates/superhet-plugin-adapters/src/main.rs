//! `superhet-plugin-adapter <pocsag|aprs|rtl433|dab|adsb|m17|dsd|tetra|ft8|ais|acars|vdl2>`: runs the external decoder for one of the
//! first-party plugins (see the library docs). Samples arrive on stdin, events leave on stdout,
//! the instance configuration is read from `SUPERHET_CONFIG`.

use std::process::ExitCode;

use serde_json::Value;
use superhet_plugin_adapters::{
    acars, adsb, ais, aprs, dab, dsd, ft8, m17, pager, rtl433,
    run::{run, run_many},
    tetra, vdl2,
};

fn config() -> Result<Value, String> {
    match std::env::var("SUPERHET_CONFIG") {
        Ok(text) if !text.trim().is_empty() => {
            serde_json::from_str(&text).map_err(|err| format!("SUPERHET_CONFIG: {err}"))
        }
        _ => Ok(Value::Null),
    }
}

/// The frequency the instance was started on (its receiver's), from the host's environment.
fn centre_hz() -> Option<u64> {
    std::env::var("SUPERHET_FREQ_HZ")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
}

fn start(mode: &str) -> Result<ExitCode, String> {
    let config = config()?;
    match mode {
        "pocsag" => {
            let config = pager::PagerConfig::from_json(&config)?;
            Ok(run_many(
                "multimon-ng",
                &config.arg_sets(),
                pager::parse_line,
            ))
        }
        "aprs" => {
            aprs::check_config(&config)?;
            // The host starts plugins in their directory; direwolf rejects config paths of
            // 100 characters or more, so keep the path relative.
            let args = ["-c", "direwolf.conf", "-r", "22050", "-t", "0", "-"].map(String::from);
            Ok(run("direwolf", &args, aprs::parse_line))
        }
        "rtl433" => {
            let protocols = rtl433::protocols_from_config(&config)?;
            Ok(run(
                "rtl_433",
                &rtl433::args(&protocols),
                rtl433::parse_line,
            ))
        }
        "dab" => {
            // Framed transport: the host passes the socket in the environment, and the block
            // the instance was started on as the frequency.
            let socket = std::env::var("SUPERHET_SOCKET")
                .map_err(|_| "SUPERHET_SOCKET is not set (framed plugin)".to_owned())?;
            let freq = std::env::var("SUPERHET_FREQ_HZ")
                .ok()
                .and_then(|v| v.parse::<u64>().ok());
            Ok(dab::run("welle-cli", &config, &socket, freq))
        }
        "adsb" => {
            let socket = std::env::var("SUPERHET_SOCKET")
                .map_err(|_| "SUPERHET_SOCKET is not set (framed plugin)".to_owned())?;
            Ok(adsb::run("readsb", &config, &socket))
        }
        "m17" => {
            let socket = std::env::var("SUPERHET_SOCKET")
                .map_err(|_| "SUPERHET_SOCKET is not set (framed plugin)".to_owned())?;
            Ok(m17::run("m17-demod", &config, &socket))
        }
        "dsd" => {
            // The tool is user-installed and may be missing: the plugin then reports that
            // instead of failing (docs/plugins-dsd.md).
            let socket = std::env::var("SUPERHET_SOCKET")
                .map_err(|_| "SUPERHET_SOCKET is not set (framed plugin)".to_owned())?;
            let dir = std::env::current_dir().map_err(|err| format!("working directory: {err}"))?;
            Ok(dsd::run(&config, &socket, &dir))
        }
        "tetra" => {
            // tetra-rx ships in the image; the ETSI codec is the user's (docs/plugins-tetra.md).
            let socket = std::env::var("SUPERHET_SOCKET")
                .map_err(|_| "SUPERHET_SOCKET is not set (framed plugin)".to_owned())?;
            Ok(tetra::run(&config, &socket))
        }
        "ft8" => {
            // FT8 / FT4 / WSPR: slots of USB audio decoded by jt9 / wsprd (docs/plugins-ft8.md).
            let config = ft8::Config::from_json(&config)?;
            let env_u64 = |key: &str| std::env::var(key).ok().and_then(|v| v.parse::<u64>().ok());
            Ok(ft8::run(
                &config,
                env_u64("SUPERHET_FREQ_HZ"),
                env_u64("SUPERHET_FT8_STREAM_START_MS"),
            ))
        }
        "ais" => ais::start("AIS-catcher", &config),
        "acars" => acars::start("acarsdec", &config, centre_hz()),
        "vdl2" => vdl2::start("dumpvdl2", &config, centre_hz()),
        other => Err(format!(
            "unknown mode {other:?} (pocsag, aprs, rtl433, dab, adsb, m17, dsd, tetra, ft8, ais, acars, vdl2)"
        )),
    }
}

fn main() -> ExitCode {
    let Some(mode) = std::env::args().nth(1) else {
        eprintln!(
            "usage: superhet-plugin-adapter <pocsag|aprs|rtl433|dab|adsb|m17|dsd|tetra|ft8|ais|acars|vdl2>"
        );
        return ExitCode::from(2);
    };
    match start(&mode) {
        Ok(code) => code,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::from(2)
        }
    }
}
