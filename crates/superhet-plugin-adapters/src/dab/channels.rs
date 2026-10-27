//! DAB Band III channel blocks (ETSI EN 300 401, ITU-R BS.1660): name and centre frequency.

/// Every Band III block, ascending: `(name, centre frequency in Hz)`.
pub const BLOCKS: [(&str, u64); 38] = [
    ("5A", 174_928_000),
    ("5B", 176_640_000),
    ("5C", 178_352_000),
    ("5D", 180_064_000),
    ("6A", 181_936_000),
    ("6B", 183_648_000),
    ("6C", 185_360_000),
    ("6D", 187_072_000),
    ("7A", 188_928_000),
    ("7B", 190_640_000),
    ("7C", 192_352_000),
    ("7D", 194_064_000),
    ("8A", 195_936_000),
    ("8B", 197_648_000),
    ("8C", 199_360_000),
    ("8D", 201_072_000),
    ("9A", 202_928_000),
    ("9B", 204_640_000),
    ("9C", 206_352_000),
    ("9D", 208_064_000),
    ("10A", 209_936_000),
    ("10B", 211_648_000),
    ("10C", 213_360_000),
    ("10D", 215_072_000),
    ("11A", 216_928_000),
    ("11B", 218_640_000),
    ("11C", 220_352_000),
    ("11D", 222_064_000),
    ("12A", 223_936_000),
    ("12B", 225_648_000),
    ("12C", 227_360_000),
    ("12D", 229_072_000),
    ("13A", 230_784_000),
    ("13B", 232_496_000),
    ("13C", 234_208_000),
    ("13D", 235_776_000),
    ("13E", 237_488_000),
    ("13F", 239_200_000),
];

/// Centre frequency of block `name` (case-insensitive).
#[must_use]
pub fn freq_of(name: &str) -> Option<u64> {
    BLOCKS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|&(_, f)| f)
}

/// The canonical (upper-case) name of block `name`.
#[must_use]
pub fn canonical(name: &str) -> Option<&'static str> {
    BLOCKS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|&(n, _)| n)
}

/// The block whose centre frequency is `freq_hz` (within 50 kHz, so a rounded value still hits).
#[must_use]
pub fn name_of(freq_hz: u64) -> Option<&'static str> {
    BLOCKS
        .iter()
        .find(|&&(_, f)| f.abs_diff(freq_hz) <= 50_000)
        .map(|&(n, _)| n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookups_work_both_ways() {
        assert_eq!(freq_of("5C"), Some(178_352_000));
        assert_eq!(freq_of("10b"), Some(211_648_000));
        assert_eq!(freq_of("14A"), None);
        assert_eq!(name_of(178_352_000), Some("5C"));
        assert_eq!(name_of(178_350_000), Some("5C"));
        assert_eq!(name_of(100_000_000), None);
        assert_eq!(canonical("13f"), Some("13F"));
    }

    #[test]
    fn the_table_is_sorted_and_spans_the_manifest_range() {
        assert!(BLOCKS.windows(2).all(|w| w[0].1 < w[1].1));
        assert_eq!(BLOCKS[0].1, 174_928_000);
        assert_eq!(BLOCKS[BLOCKS.len() - 1].1, 239_200_000);
        // Letter A..D blocks of one number are 1.712 MHz apart.
        assert_eq!(BLOCKS[1].1 - BLOCKS[0].1, 1_712_000);
    }
}
