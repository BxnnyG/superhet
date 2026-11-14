//! Picking callsigns and the locator out of an FT8 / FT4 / WSPR message.
//!
//! The messages of these modes are a handful of fixed shapes (`CQ DX DL1ABC JO62`,
//! `DL1ABC K1ABC -12`, `K1ABC FN42 37`, ...). This is not a full message-type decoder: it only
//! recognises the common shapes and leaves everything else (free text, telemetry, contest
//! exchanges) as a plain message.

/// Who a message is from and to, and where the sender is.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Parties {
    /// The calling or answering station (`CQ` caller, or the second callsign of a directed message).
    pub callsign: Option<String>,
    /// The station a directed message is addressed to (absent for `CQ`).
    pub to: Option<String>,
    /// Maidenhead locator (4 or 6 characters) when the message carries one.
    pub grid: Option<String>,
    /// A general call (`CQ ...`).
    pub cq: bool,
}

/// True for something that looks like an amateur callsign: letters and digits (and `/` for a
/// prefix or suffix), at least one digit, ending in a letter.
/// A hashed call in angle brackets counts, `<...>` (a call the decoder could not resolve) does not.
#[must_use]
pub fn is_callsign(token: &str) -> bool {
    let token = token.strip_prefix('<').unwrap_or(token);
    let token = token.strip_suffix('>').unwrap_or(token);
    if token.len() < 3 || token.len() > 13 {
        return false;
    }
    if !token
        .bytes()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'/')
    {
        return false;
    }
    // The longest part between slashes is the call proper (`PJ4/K1ABC`, `K1ABC/P`).
    let base = token.split('/').max_by_key(|part| part.len()).unwrap_or("");
    // A call ends in a letter; a locator (`JO62`) or a sign-off (`RR73`) ends in a digit.
    base.len() >= 3
        && base.bytes().any(|b| b.is_ascii_digit())
        && base.bytes().last().is_some_and(|b| b.is_ascii_uppercase())
}

/// True for a 4 or 6 character Maidenhead locator (`JO62`, `JO62QM`). `RR73`, the FT8 sign-off,
/// has the shape of a locator and is not one.
#[must_use]
pub fn is_grid(token: &str) -> bool {
    let b = token.as_bytes();
    if token == "RR73" || !(b.len() == 4 || b.len() == 6) {
        return false;
    }
    let field = |c: u8| (b'A'..=b'R').contains(&c);
    let square = |c: u8| c.is_ascii_digit();
    let subsquare = |c: u8| (b'A'..=b'X').contains(&c);
    field(b[0])
        && field(b[1])
        && square(b[2])
        && square(b[3])
        && (b.len() == 4 || (subsquare(b[4]) && subsquare(b[5])))
}

fn plain(token: &str) -> String {
    token
        .trim_start_matches('<')
        .trim_end_matches('>')
        .to_owned()
}

/// The parties of an FT8 / FT4 message.
#[must_use]
pub fn parties(message: &str) -> Parties {
    let tokens: Vec<&str> = message.split_whitespace().collect();
    let mut out = Parties::default();
    let Some(&first) = tokens.first() else {
        return out;
    };
    if first == "CQ" {
        out.cq = true;
        // `CQ DX DL1ABC JO62`, `CQ POTA K1ABC FN42`, `CQ 123 ...`: the caller is the first
        // callsign-like word after the optional modifier.
        if let Some(at) = tokens.iter().skip(1).take(2).position(|t| is_callsign(t)) {
            let at = at + 1;
            out.callsign = Some(plain(tokens[at]));
            out.grid = tokens
                .get(at + 1)
                .filter(|t| is_grid(t))
                .map(|t| (*t).to_owned());
        }
        return out;
    }
    if tokens.len() >= 2 && is_callsign(tokens[1]) && (is_callsign(first) || first.starts_with('<'))
    {
        out.to = is_callsign(first).then(|| plain(first));
        out.callsign = Some(plain(tokens[1]));
        out.grid = tokens
            .get(2)
            .filter(|t| is_grid(t))
            .map(|t| (*t).to_owned());
    }
    out
}

/// A WSPR power (dBm) as watts text for the summary line: `0.2 W`, `5 W`.
#[must_use]
pub fn watts(dbm: i32) -> String {
    let w = 10f64.powf((f64::from(dbm) - 30.0) / 10.0);
    if w >= 10.0 {
        format!("{w:.0} W")
    } else if w >= 1.0 {
        format!("{w:.1} W")
    } else if w >= 0.1 {
        format!("{w:.2} W")
    } else {
        format!("{:.0} mW", w * 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(message: &str) -> (Option<String>, Option<String>, Option<String>, bool) {
        let x = parties(message);
        (x.callsign, x.to, x.grid, x.cq)
    }

    fn s(text: &str) -> Option<String> {
        Some(text.to_owned())
    }

    #[test]
    fn callsigns() {
        for ok in [
            "K1ABC",
            "DL1ABC",
            "9A1AA",
            "3DA0XYZ",
            "PJ4/K1ABC",
            "K1ABC/P",
            "<DL1ABC>",
        ] {
            assert!(is_callsign(ok), "{ok}");
        }
        for bad in [
            "CQ", "DX", "JO62", "RR73", "73", "-12", "R-05", "<...>", "TEST", "AB", "k1abc",
        ] {
            assert!(!is_callsign(bad), "{bad}");
        }
    }

    #[test]
    fn grids() {
        assert!(is_grid("JO62"));
        assert!(is_grid("FN42"));
        assert!(is_grid("JO62QM"));
        assert!(!is_grid("RR73"));
        assert!(!is_grid("73"));
        assert!(!is_grid("SO62"));
        assert!(!is_grid("JO6"));
        assert!(!is_grid("JO62ZZ"));
    }

    #[test]
    fn cq_messages() {
        assert_eq!(p("CQ DL1ABC JO62"), (s("DL1ABC"), None, s("JO62"), true));
        assert_eq!(p("CQ DX K1ABC FN42"), (s("K1ABC"), None, s("FN42"), true));
        assert_eq!(p("CQ POTA W1AW FN31"), (s("W1AW"), None, s("FN31"), true));
        assert_eq!(p("CQ K1ABC"), (s("K1ABC"), None, None, true));
        assert_eq!(p("CQ TEST"), (None, None, None, true));
    }

    #[test]
    fn directed_messages() {
        assert_eq!(
            p("DL1ABC K1ABC FN42"),
            (s("K1ABC"), s("DL1ABC"), s("FN42"), false)
        );
        assert_eq!(
            p("DL1ABC K1ABC -12"),
            (s("K1ABC"), s("DL1ABC"), None, false)
        );
        assert_eq!(
            p("DL1ABC K1ABC RR73"),
            (s("K1ABC"), s("DL1ABC"), None, false)
        );
        assert_eq!(
            p("<DL1ABC> K1ABC R-05"),
            (s("K1ABC"), s("DL1ABC"), None, false)
        );
        // A call the decoder could not resolve: the sender is still known.
        assert_eq!(p("<...> K1ABC 73"), (s("K1ABC"), None, None, false));
    }

    #[test]
    fn other_messages_stay_plain() {
        assert_eq!(p("TNX BOB 73 GL"), (None, None, None, false));
        assert_eq!(p(""), (None, None, None, false));
        assert_eq!(p("K1ABC"), (None, None, None, false));
    }

    #[test]
    fn power_text() {
        assert_eq!(watts(37), "5.0 W");
        assert_eq!(watts(30), "1.0 W");
        assert_eq!(watts(23), "0.20 W");
        assert_eq!(watts(10), "10 mW");
        assert_eq!(watts(40), "10 W");
    }
}
