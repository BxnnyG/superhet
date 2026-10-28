//! `superhet-plugin-adapter <pocsag|aprs|rtl433|dab|adsb>`: runs the external decoder for one of the
//! first-party plugins (see the library docs). Samples arrive on stdin, events leave on stdout,
//! the instance configuration is read from `SUPERHET_CONFIG`.

use std::process::ExitCode;

use serde_json::Value;
use superhet_plugin_adapters::{
    adsb, aprs, dab, pager, rtl433,
    run::{run, run_many},
};

fn config() -> Result<Value, String> {
    match std::env::var("SUPERHET_CONFIG") {
        Ok(text) if !text.trim().is_empty() => {
            serde_json::from_str(&text).map_err(|err| format!("SUPERHET_CONFIG: {err}"))
        }
        _ => Ok(Value::Null),
    }
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
        other => Err(format!(
            "unknown mode {other:?} (pocsag, aprs, rtl433, dab, adsb)"
        )),
    }
}

fn main() -> ExitCode {
    let Some(mode) = std::env::args().nth(1) else {
        eprintln!("usage: superhet-plugin-adapter <pocsag|aprs|rtl433|dab|adsb>");
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
