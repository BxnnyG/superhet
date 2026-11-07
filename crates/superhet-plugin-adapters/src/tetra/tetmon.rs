//! The datagrams `tetra-rx` of osmo-tetra-sq5bpf sends when `TETRA_HACK_PORT` is set (the
//! interface of the "telive" receiver), and the few lines of its console output the adapter reads.
//!
//! Layouts are taken from the sources (`src/tetra_upper_mac.c`, `src/lower_mac/tetra_lower_mac.c`,
//! `src/tetra_sds.c` at the commit pinned in the Dockerfile); `NETINFO1` and `ENCINFO1` were also
//! captured from a real `tetra-rx` run (`tests/tetra/transcripts/generator-sync.udp`).
//!
//! Text datagrams: `TETMON_begin FUNC:<name> KEY:VALUE ... RX:0 TETMON_end`, NUL-terminated, in
//! some cases padded with unspecified bytes after the NUL. Voice datagram: `TRA:<usage marker,
//! hex> RX:<id>\0` (13 bytes) followed by 690 native-endian `int16` values, the ETSI channel
//! decoder's input block of one traffic slot.

/// Size of the voice block after the 13-byte header: 690 `int16`.
pub const VOICE_BLOCK_BYTES: usize = 690 * 2;
const VOICE_HEADER: usize = 13;

/// A parsed datagram.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// A burst was received (sent for every one).
    Burst,
    /// Cell identity from the synchronisation burst, with the frequencies and local area of the
    /// last system information.
    NetInfo {
        colour_code: u8,
        mcc: u16,
        mnc: u16,
        dl_hz: u32,
        ul_hz: u32,
        la: u16,
        /// 0 unknown, 1 air interface encryption off, 2 on (from the system information).
        crypt: u8,
    },
    /// Cell level encryption state and which MAC encryption modes were seen (bit n = mode n).
    EncInfo {
        crypt: u8,
        seen: u8,
    },
    /// Neighbour cell (D-NWRK-BROADCAST) or the carrier of the serving cell.
    FreqInfoCell {
        dl_hz: u32,
        ul_hz: Option<u32>,
        mcc: Option<u16>,
        mnc: Option<u16>,
        la: Option<u16>,
    },
    /// Downlink carrier a traffic channel was allocated on.
    FreqInfoChannel {
        dl_hz: u32,
    },
    Setup {
        usage: u32,
        called: u32,
        calling: u32,
        cid: u32,
    },
    Connect {
        called: u32,
        usage: u32,
        cid: u32,
    },
    TxGranted {
        called: u32,
        usage: u32,
        cid: u32,
        /// The transmitting party, when the PDU names it.
        talker: Option<u32>,
    },
    Release {
        called: u32,
        cid: u32,
        reason: Option<String>,
    },
    Status {
        called: u32,
        calling: u32,
        status: u32,
    },
    /// Decoded short data (`SDSDEC`): the decoder's own description.
    Sds {
        called: Option<u32>,
        calling: Option<u32>,
        text: Option<String>,
        protocol: Option<String>,
        info: String,
    },
    /// Any other CMCE PDU the decoder recognised: the name and the address it came with.
    Pdu {
        name: String,
        ssi: u32,
        usage: u32,
        encrypted: bool,
    },
    /// One traffic slot of voice.
    Voice {
        usage: u32,
        block: Vec<u8>,
    },
}

/// Parse one datagram; `None` for anything that is not understood.
#[must_use]
pub fn parse_datagram(data: &[u8]) -> Option<Message> {
    if data.starts_with(b"TRA:") {
        let usage = std::str::from_utf8(data.get(4..6)?)
            .ok()
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())?;
        let block = data.get(VOICE_HEADER..VOICE_HEADER + VOICE_BLOCK_BYTES)?;
        return Some(Message::Voice {
            usage,
            block: block.to_vec(),
        });
    }
    let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
    let text = String::from_utf8_lossy(&data[..end]);
    parse_text(&text)
}

fn parse_text(text: &str) -> Option<Message> {
    let body = text.trim().strip_prefix("TETMON_begin ")?;
    let body = body.strip_suffix("TETMON_end").unwrap_or(body).trim();
    let function_text = body.strip_prefix("FUNC:")?;
    // The short data description is free text with brackets and colons of its own.
    for prefix in ["SDSDEC [", "SDS ["] {
        if let Some(rest) = function_text.strip_prefix(prefix) {
            if prefix == "SDS [" {
                // The raw bit dump of the PDU; SDSDEC follows with the decoded form.
                return None;
            }
            let inner = rest
                .rfind("] RX:")
                .map_or_else(|| rest.trim_end_matches(']'), |at| &rest[..at]);
            return Some(parse_sds(inner));
        }
    }
    let fields = Fields::new(function_text);
    let int = |key: &str| fields.get(key).and_then(|v| v.parse::<u32>().ok());
    let hex = |key: &str| {
        fields
            .get(key)
            .and_then(|v| u32::from_str_radix(v, 16).ok())
    };
    let small = |value: Option<u32>| value.and_then(|v| u16::try_from(v).ok());
    match fields.function.as_str() {
        "BURST" => Some(Message::Burst),
        "NETINFO1" => Some(Message::NetInfo {
            colour_code: u8::try_from(hex("CCODE")?).ok()?,
            mcc: small(hex("MCC"))?,
            mnc: small(hex("MNC"))?,
            dl_hz: int("DLF").unwrap_or(0),
            ul_hz: int("ULF").unwrap_or(0),
            la: small(int("LA")).unwrap_or(0),
            crypt: u8::try_from(int("CRYPT").unwrap_or(0)).ok()?,
        }),
        "ENCINFO1" => Some(Message::EncInfo {
            crypt: u8::try_from(int("CRYPT").unwrap_or(0)).ok()?,
            seen: u8::try_from(hex("ENC").unwrap_or(0)).ok()?,
        }),
        // Neighbour cells carry the identity in hexadecimal like NETINFO1 (`%4.4x`), the
        // local area in decimal.
        "FREQINFO1" => Some(Message::FreqInfoCell {
            dl_hz: int("DLF")?,
            ul_hz: int("ULF"),
            mcc: small(hex("MCC")),
            mnc: small(hex("MNC")),
            la: small(int("LA")),
        }),
        "FREQINFO2" => Some(Message::FreqInfoChannel { dl_hz: int("DLF")? }),
        "DSETUPDEC" => Some(Message::Setup {
            usage: int("IDX").unwrap_or(0),
            called: int("SSI")?,
            calling: int("SSI2").unwrap_or(0),
            cid: int("CID")?,
        }),
        "DCONNECTDEC" => Some(Message::Connect {
            called: int("SSI")?,
            usage: int("IDX").unwrap_or(0),
            cid: int("CID")?,
        }),
        "DTXGRANTDEC" => Some(Message::TxGranted {
            called: int("SSI")?,
            usage: int("IDX").unwrap_or(0),
            cid: int("CID")?,
            talker: int("SSI2"),
        }),
        "DRELEASEDEC" => Some(Message::Release {
            called: int("SSI")?,
            cid: int("CID")?,
            reason: fields.bracket.clone(),
        }),
        "DSTATUSDEC" => Some(Message::Status {
            called: int("SSI")?,
            calling: int("SSI2").unwrap_or(0),
            status: int("STATUS")?,
        }),
        "AFCVAL" => None,
        name => {
            // The generic line: `FUNC:<PDU name> SSI:%8.8i IDX:%3.3i IDT:%i ENCR:%i`.
            let ssi = int("SSI")?;
            Some(Message::Pdu {
                name: name.to_owned(),
                ssi,
                usage: int("IDX").unwrap_or(0),
                encrypted: int("ENCR").unwrap_or(0) != 0,
            })
        }
    }
}

/// `KEY:VALUE` pairs after the function name. The name itself may contain spaces
/// (`D-TX CEASED`), and a pair may be followed by a bracketed free text (`NID:5 [reason]`).
struct Fields {
    function: String,
    pairs: Vec<(String, String)>,
    bracket: Option<String>,
}

impl Fields {
    fn new(text: &str) -> Self {
        let (head, bracket) = match (text.find('['), text.rfind(']')) {
            (Some(open), Some(close)) if open < close => (
                format!("{}{}", &text[..open], &text[close + 1..]),
                Some(text[open + 1..close].trim().to_owned()),
            ),
            _ => (text.to_owned(), None),
        };
        let mut function = Vec::new();
        let mut pairs = Vec::new();
        for token in head.split_whitespace() {
            match token.split_once(':') {
                Some((key, value))
                    if !key.is_empty()
                        && key
                            .chars()
                            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) =>
                {
                    pairs.push((key.to_owned(), value.to_owned()));
                }
                _ if pairs.is_empty() => function.push(token),
                _ => {}
            }
        }
        // The first token is `FUNC:` stripped already, so the first pair-less words are the name.
        Self {
            function: function.join(" "),
            pairs,
            bracket,
        }
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// `CPTI:1 CalledSSI:... CallingSSI:... CallingEXT:... UserData4: len:.. protoid:82(TEXT ..)
/// SDS-TL:[...] DATA:[text]` (see `sds.c`); everything is optional.
fn parse_sds(description: &str) -> Message {
    let number = |label: &str| {
        description.find(label).and_then(|at| {
            let rest = &description[at + label.len()..];
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u32>().ok()
        })
    };
    let protocol = description.find("protoid:").map(|at| {
        let rest = &description[at + "protoid:".len()..];
        // `82(Text messaging)`: the number and the name, as printed.
        match rest.find(')') {
            Some(close) => rest[..=close].to_owned(),
            None => rest.split_whitespace().next().unwrap_or("").to_owned(),
        }
    });
    let text = description.find("DATA:[").map(|at| {
        let rest = &description[at + "DATA:[".len()..];
        rest.rfind(']')
            .map_or(rest, |close| &rest[..close])
            .trim()
            .to_owned()
    });
    let info: String = description
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(300)
        .collect();
    Message::Sds {
        called: number("CalledSSI:"),
        calling: number("CallingSSI:").filter(|v| *v != 0),
        text: text.filter(|t| !t.is_empty()),
        protocol,
        info: info.trim().to_owned(),
    }
}

/// What the adapter reads from `tetra-rx`'s console: the system information line, which holds the
/// service details the datagrams leave out.
///
/// `BNCH SYSINFO (DL 392000000 Hz, UL 382000000 Hz), service_details 0x00e2 LA:1`
#[must_use]
pub fn parse_sysinfo_line(line: &str) -> Option<SysInfo> {
    let rest = &line[line.find("BNCH SYSINFO (DL ")? + "BNCH SYSINFO (DL ".len()..];
    let (dl, rest) = rest.split_once(" Hz, UL ")?;
    let (ul, rest) = rest.split_once(" Hz), service_details 0x")?;
    let (details, rest) = rest.split_once(" LA:")?;
    let la: String = rest.chars().take_while(char::is_ascii_digit).collect();
    Some(SysInfo {
        dl_hz: dl.trim().parse().ok()?,
        ul_hz: ul.trim().parse().ok()?,
        service_details: u16::from_str_radix(details.trim(), 16).ok()?,
        la: la.parse().ok()?,
    })
}

/// A system information broadcast as `tetra-rx` prints it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SysInfo {
    pub dl_hz: u32,
    pub ul_hz: u32,
    pub service_details: u16,
    pub la: u16,
}

/// Names of the BS service details bits (EN 300 392-2 clause 21.4.4.1), as `tetra-rx` numbers them.
#[must_use]
pub fn service_names(details: u16) -> Vec<&'static str> {
    const BITS: [(u16, &str); 11] = [
        (11, "registration-required"),
        (10, "deregistration-required"),
        (9, "priority-cell"),
        (8, "minimum-mode"),
        (7, "migration"),
        (6, "system-wide-services"),
        (5, "voice"),
        (4, "circuit-mode-data"),
        (2, "sndcp"),
        (1, "air-interface-encryption"),
        (0, "advanced-link"),
    ];
    BITS.iter()
        .filter(|(bit, _)| details & (1 << bit) != 0)
        .map(|(_, name)| *name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &str) -> Option<Message> {
        parse_datagram(line.as_bytes())
    }

    #[test]
    fn netinfo_has_its_identity_in_hex_of_the_decimal_value() {
        // Captured from tetra-rx: MCC 262, MNC 42.
        let m = text(
            "TETMON_begin FUNC:NETINFO1 CCODE:00 MCC:0106 MNC:002a DLF:0 ULF:0 LA:0 CRYPT:0 RX:0 TETMON_end",
        );
        assert_eq!(
            m,
            Some(Message::NetInfo {
                colour_code: 0,
                mcc: 262,
                mnc: 42,
                dl_hz: 0,
                ul_hz: 0,
                la: 0,
                crypt: 0
            })
        );
    }

    #[test]
    fn calls_and_their_parties() {
        assert_eq!(
            text(
                "TETMON_begin FUNC:DSETUPDEC IDX:5 SSI:9001 SSI2:2630001 CID:1234 NID:0 RX:0 TETMON_end"
            ),
            Some(Message::Setup {
                usage: 5,
                called: 9001,
                calling: 2_630_001,
                cid: 1234
            })
        );
        assert_eq!(
            text(
                "TETMON_begin FUNC:DTXGRANTDEC SSI:9001 IDX:5 CID:1234 TXGRANT:0 TXPERM:0 ENCC:0 TPTI:1 SSI2:2630002 RX:0 TETMON_end"
            ),
            Some(Message::TxGranted {
                called: 9001,
                usage: 5,
                cid: 1234,
                talker: Some(2_630_002)
            })
        );
        assert_eq!(
            text(
                "TETMON_begin FUNC:DRELEASEDEC SSI:9001 CID:1234 NID:12 [Call time-out] RX:0 TETMON_end"
            ),
            Some(Message::Release {
                called: 9001,
                cid: 1234,
                reason: Some("Call time-out".into())
            })
        );
        assert_eq!(
            text(
                "TETMON_begin FUNC:DCONNECTDEC SSI:9001 IDX:5 CID:1234 CALLOWN:0 NID:3 [x y] RX:0 TETMON_end"
            ),
            Some(Message::Connect {
                called: 9001,
                usage: 5,
                cid: 1234
            })
        );
    }

    #[test]
    fn generic_pdus_keep_names_with_spaces() {
        assert_eq!(
            text("TETMON_begin FUNC:D-TX CEASED SSI:00009001 IDX:005 IDT:4 ENCR:0 RX:0 TETMON_end"),
            Some(Message::Pdu {
                name: "D-TX CEASED".into(),
                ssi: 9001,
                usage: 5,
                encrypted: false
            })
        );
        let m = text("TETMON_begin FUNC:D-SETUP SSI:00009001 IDX:005 IDT:4 ENCR:2 RX:0 TETMON_end");
        assert!(
            matches!(
                m,
                Some(Message::Pdu {
                    encrypted: true,
                    ..
                })
            ),
            "{m:?}"
        );
    }

    #[test]
    fn neighbours_status_and_encryption_info() {
        assert_eq!(
            text(
                "TETMON_begin FUNC:FREQINFO1 DLF:392012500 ULF:382012500 MCC:0106 MNC:002a LA:3 RX:0 TETMON_end"
            ),
            Some(Message::FreqInfoCell {
                dl_hz: 392_012_500,
                ul_hz: Some(382_012_500),
                mcc: Some(262),
                mnc: Some(42),
                la: Some(3)
            })
        );
        assert_eq!(
            text("TETMON_begin FUNC:DSTATUSDEC SSI:9001 SSI2:2630001 STATUS:32770 RX:0 TETMON_end"),
            Some(Message::Status {
                called: 9001,
                calling: 2_630_001,
                status: 32770
            })
        );
        assert_eq!(
            text("TETMON_begin FUNC:ENCINFO1 CRYPT:2 ENC:03 RX:0 TETMON_end"),
            Some(Message::EncInfo { crypt: 2, seen: 3 })
        );
    }

    #[test]
    fn short_data_is_described_by_the_decoder() {
        let m = text(
            "TETMON_begin FUNC:SDSDEC [CPTI:1 CalledSSI:9001 CallingSSI:2630001 CallingEXT:0  UserData4: len:48 protoid:82(TEXT) SDS-TL:[ MsgType:SDS-TRANSFER MSG_REF:3 TO_GROUP:1 ]  coding_scheme:01 DATA:[Hello team]] RX:0 TETMON_end",
        );
        let Some(Message::Sds {
            called,
            calling,
            text: body,
            protocol,
            ..
        }) = m
        else {
            panic!("{m:?}")
        };
        assert_eq!((called, calling), (Some(9001), Some(2_630_001)));
        assert_eq!(body.as_deref(), Some("Hello team"));
        assert_eq!(protocol.as_deref(), Some("82(TEXT)"));
        // The raw bit dump is not an event.
        assert_eq!(text("TETMON_begin FUNC:SDS [0101] TETMON_end"), None);
    }

    #[test]
    fn voice_datagrams_carry_the_usage_marker_and_one_block() {
        let mut datagram = b"TRA:0a RX:00\0".to_vec();
        datagram.extend((0..VOICE_BLOCK_BYTES).map(|i| (i % 251) as u8));
        let Some(Message::Voice { usage, block }) = parse_datagram(&datagram) else {
            panic!("not a voice datagram")
        };
        assert_eq!(usage, 10);
        assert_eq!(block.len(), VOICE_BLOCK_BYTES);
        assert_eq!(block[3], 3);
        // Too short.
        assert_eq!(parse_datagram(&datagram[..100]), None);
    }

    #[test]
    fn garbage_and_padding_are_tolerated() {
        assert_eq!(text("hello"), None);
        assert_eq!(text("TETMON_begin FUNC:NETINFO1 TETMON_end"), None);
        let mut padded = b"TETMON_begin FUNC:BURST RX:0 TETMON_end\0".to_vec();
        padded.extend([0xAA; 80]);
        assert_eq!(parse_datagram(&padded), Some(Message::Burst));
    }

    #[test]
    fn the_system_information_line_gives_the_service_details() {
        let line = "BNCH SYSINFO (DL 392012500 Hz, UL 382012500 Hz), service_details 0x0262 LA:1 Hyperframe 4";
        let info = parse_sysinfo_line(line).expect("sysinfo");
        assert_eq!(
            (info.dl_hz, info.ul_hz, info.service_details, info.la),
            (392_012_500, 382_012_500, 0x0262, 1)
        );
        assert_eq!(
            service_names(0x0262),
            [
                "priority-cell",
                "system-wide-services",
                "voice",
                "air-interface-encryption"
            ]
        );
        assert_eq!(parse_sysinfo_line("something else"), None);
    }
}
