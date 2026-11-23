//! The weather satellites Superhet knows, with what is known about their transmissions.
//!
//! Status as verified on 2026-11-23 (ADR-0034): NOAA-15, -18 and -19 were switched off in 2025 and
//! no longer transmit APT or HRPT; Meteor-M N2-2 stopped sending images after a micrometeoroid
//! hit in December 2019; Meteor-M N2-3 and N2-4 are the LRPT sources that remain. Satellites
//! change state without notice: the table says what was true when it was written, not what is.

use serde::Serialize;

/// Whether a satellite can be received.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// Transmitting images as far as the sources below say.
    Operational,
    /// Switched off by its operator; stays in orbit, silent.
    Decommissioned,
    /// In orbit but without image downlink.
    NoImages,
}

/// One downlink frequency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Downlink {
    pub hz: u64,
    pub label: &'static str,
}

/// A weather satellite.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Entry {
    /// NORAD catalogue number.
    pub norad_id: u64,
    /// Name as CelesTrak writes it.
    pub name: &'static str,
    /// Image format on the downlink.
    pub format: &'static str,
    pub status: Status,
    /// Short statement of what happened or what to expect.
    pub note: &'static str,
    pub downlinks: &'static [Downlink],
}

const METEOR_LRPT: &[Downlink] = &[
    Downlink {
        hz: 137_900_000,
        label: "LRPT 137.900 MHz",
    },
    Downlink {
        hz: 137_100_000,
        label: "LRPT 137.100 MHz",
    },
];

/// All entries, operational ones first.
pub const CATALOGUE: &[Entry] = &[
    Entry {
        norad_id: 59051,
        name: "METEOR-M2 4",
        format: "LRPT",
        status: Status::Operational,
        note: "Transmits LRPT on 137.900 MHz and, at times, 137.100 MHz; the active frequency changes. Launched 2024.",
        downlinks: METEOR_LRPT,
    },
    Entry {
        norad_id: 57166,
        name: "METEOR-M2 3",
        format: "LRPT",
        status: Status::Operational,
        note: "Transmits LRPT on 137.900 MHz and, at times, 137.100 MHz; the active frequency changes. Launched 2023.",
        downlinks: METEOR_LRPT,
    },
    Entry {
        norad_id: 44387,
        name: "METEOR-M2 2",
        format: "LRPT",
        status: Status::NoImages,
        note: "Image downlink ceased on 2019-12-18 after a micrometeoroid impact depressurised the spacecraft.",
        downlinks: &[],
    },
    Entry {
        norad_id: 33591,
        name: "NOAA 19",
        format: "APT",
        status: Status::Decommissioned,
        note: "Decommissioned by NOAA on 2025-08-13 (battery failure); transmitters shut down.",
        downlinks: &[],
    },
    Entry {
        norad_id: 28654,
        name: "NOAA 18",
        format: "APT",
        status: Status::Decommissioned,
        note: "Decommissioned by NOAA on 2025-06-06; transmitters shut down.",
        downlinks: &[],
    },
    Entry {
        norad_id: 25338,
        name: "NOAA 15",
        format: "APT",
        status: Status::Decommissioned,
        note: "Decommissioned by NOAA on 2025-08-19; transmitters shut down.",
        downlinks: &[],
    },
];

/// Satellites whose passes are worth predicting.
pub fn operational() -> impl Iterator<Item = &'static Entry> {
    CATALOGUE.iter().filter(|e| e.status == Status::Operational)
}

/// The entry for a NORAD number.
#[must_use]
pub fn by_norad(norad_id: u64) -> Option<&'static Entry> {
    CATALOGUE.iter().find(|e| e.norad_id == norad_id)
}

/// A name reduced to its letters and digits, upper case: "Meteor-M 2-4", "METEOR-M2 4" and
/// "METEOR-M2-4" all become "METEORM24".
fn squash(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// The entry a satellite name from SatDump, CelesTrak or the user refers to.
#[must_use]
pub fn by_name(name: &str) -> Option<&'static Entry> {
    let wanted = squash(name);
    if wanted.is_empty() {
        return None;
    }
    CATALOGUE.iter().find(|e| squash(e.name) == wanted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_however_they_are_spelled() {
        for spelling in [
            "METEOR-M2 4",
            "Meteor-M2-4",
            "meteor m2 4",
            "METEOR-M 2-4",
            " METEORM24 ",
        ] {
            assert_eq!(
                by_name(spelling).map(|e| e.norad_id),
                Some(59051),
                "{spelling}"
            );
        }
        assert_eq!(by_name("NOAA-19").map(|e| e.norad_id), Some(33591));
        assert!(by_name("").is_none());
        assert!(by_name("Unknown Meteor").is_none());
        assert_eq!(by_norad(57166).map(|e| e.name), Some("METEOR-M2 3"));
    }

    #[test]
    fn only_meteor_m2_3_and_4_are_operational() {
        let ids: Vec<u64> = operational().map(|e| e.norad_id).collect();
        assert_eq!(ids, [59051, 57166]);
        assert!(operational().all(|e| !e.downlinks.is_empty()));
        assert!(
            CATALOGUE
                .iter()
                .filter(|e| e.status == Status::Decommissioned)
                .all(|e| e.downlinks.is_empty() && e.name.starts_with("NOAA"))
        );
    }
}
