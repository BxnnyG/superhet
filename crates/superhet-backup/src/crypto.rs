//! Passphrase encryption of a backup file (ADR-0030).
//!
//! Layout: a 43-byte header, then the plaintext in 64 KiB chunks sealed with AES-256-GCM.
//!
//! ```text
//! "SHBKENC1" | m_kib u32 | t u32 | p u32 | salt[16] | nonce_prefix[7]      (all big-endian)
//! chunk i:   AES-256-GCM(key, nonce = prefix | i u32 | last u8, aad = header, plaintext)
//! ```
//!
//! The key is Argon2id(passphrase, salt, m, t, p). The nonce carries the chunk counter and a
//! "last chunk" flag (the STREAM construction), so reordering, dropping or truncating chunks
//! fails authentication; the header is bound as associated data. Argon2 parameters read from a
//! file are capped, so a crafted header cannot make the server allocate gigabytes.

use std::io::{self, Read, Write};

use argon2::{Algorithm, Argon2, Params, Version};
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};

use crate::Error;

/// First bytes of an encrypted backup.
pub const MAGIC: &[u8; 8] = b"SHBKENC1";
const SALT_LEN: usize = 16;
const PREFIX_LEN: usize = 7;
const HEADER_LEN: usize = 8 + 4 + 4 + 4 + SALT_LEN + PREFIX_LEN;
/// Plaintext bytes per chunk.
const CHUNK: usize = 64 * 1024;
const TAG: usize = 16;

/// Argon2id cost written by this version: 64 MiB, 3 passes, 1 lane (about a quarter second).
const M_KIB: u32 = 64 * 1024;
const T_COST: u32 = 3;
const P_COST: u32 = 1;
/// Upper bounds accepted from a file.
const M_KIB_MAX: u32 = 256 * 1024;
const T_COST_MAX: u32 = 12;
const P_COST_MAX: u32 = 4;

/// Shortest accepted passphrase (characters).
pub const PASSPHRASE_MIN: usize = 8;

/// `true` if `head` (the first bytes of a file) starts like an encrypted backup.
#[must_use]
pub fn is_encrypted(head: &[u8]) -> bool {
    head.starts_with(MAGIC)
}

/// Check a passphrase against the policy.
///
/// # Errors
/// [`Error::Invalid`] if it is too short or too long.
pub fn check_passphrase(passphrase: &str) -> Result<(), Error> {
    let n = passphrase.chars().count();
    if n < PASSPHRASE_MIN {
        return Err(Error::Invalid(format!(
            "the passphrase needs at least {PASSPHRASE_MIN} characters"
        )));
    }
    if passphrase.len() > 1024 {
        return Err(Error::Invalid("the passphrase is too long".into()));
    }
    Ok(())
}

fn derive(passphrase: &str, salt: &[u8], m: u32, t: u32, p: u32) -> Result<[u8; 32], Error> {
    let params = Params::new(m, t, p, Some(32)).map_err(|e| Error::BadArchive(e.to_string()))?;
    let mut key = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| Error::BadArchive(e.to_string()))?;
    Ok(key)
}

fn nonce(prefix: &[u8; PREFIX_LEN], counter: u32, last: bool) -> Nonce {
    let mut n = [0u8; NONCE_LEN];
    n[..PREFIX_LEN].copy_from_slice(prefix);
    n[PREFIX_LEN..PREFIX_LEN + 4].copy_from_slice(&counter.to_be_bytes());
    n[NONCE_LEN - 1] = u8::from(last);
    Nonce::assume_unique_for_key(n)
}

fn aead_key(key: &[u8; 32]) -> Result<LessSafeKey, Error> {
    UnboundKey::new(&AES_256_GCM, key)
        .map(LessSafeKey::new)
        .map_err(|_| Error::BadArchive("cipher setup failed".into()))
}

/// Fill `buf` as far as the reader goes; returns the number of bytes read (`< buf.len()` only at
/// the end of the input).
fn read_full(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Encrypt everything `input` yields into `output`.
///
/// # Errors
/// Fails on I/O errors or a passphrase outside the policy.
pub fn encrypt(
    input: &mut impl Read,
    output: &mut impl Write,
    passphrase: &str,
) -> Result<(), Error> {
    check_passphrase(passphrase)?;
    let mut salt = [0u8; SALT_LEN];
    let mut prefix = [0u8; PREFIX_LEN];
    getrandom::fill(&mut salt).map_err(|e| Error::Io(io::Error::other(e.to_string())))?;
    getrandom::fill(&mut prefix).map_err(|e| Error::Io(io::Error::other(e.to_string())))?;
    let mut header = Vec::with_capacity(HEADER_LEN);
    header.extend_from_slice(MAGIC);
    header.extend_from_slice(&M_KIB.to_be_bytes());
    header.extend_from_slice(&T_COST.to_be_bytes());
    header.extend_from_slice(&P_COST.to_be_bytes());
    header.extend_from_slice(&salt);
    header.extend_from_slice(&prefix);
    output.write_all(&header)?;
    let key = aead_key(&derive(passphrase, &salt, M_KIB, T_COST, P_COST)?)?;

    let mut cur = vec![0u8; CHUNK];
    let mut next = vec![0u8; CHUNK];
    let mut cur_len = read_full(input, &mut cur)?;
    let mut counter: u32 = 0;
    loop {
        // Look one chunk ahead: the chunk before the end of the input carries the last flag.
        let next_len = if cur_len == CHUNK {
            read_full(input, &mut next)?
        } else {
            0
        };
        let last = next_len == 0;
        let mut sealed = cur[..cur_len].to_vec();
        key.seal_in_place_append_tag(
            nonce(&prefix, counter, last),
            Aad::from(&header),
            &mut sealed,
        )
        .map_err(|_| Error::BadArchive("encryption failed".into()))?;
        output.write_all(&sealed)?;
        if last {
            return Ok(());
        }
        counter = counter
            .checked_add(1)
            .ok_or_else(|| Error::Invalid("the backup is too large to encrypt".into()))?;
        std::mem::swap(&mut cur, &mut next);
        cur_len = next_len;
    }
}

/// Decrypt `input` into `output`, writing at most `max_plain` bytes.
///
/// # Errors
/// [`Error::WrongPassphrase`] if authentication fails (wrong passphrase or modified file),
/// [`Error::BadArchive`] for a malformed header or a result above `max_plain`.
pub fn decrypt(
    input: &mut impl Read,
    output: &mut impl Write,
    passphrase: &str,
    max_plain: u64,
) -> Result<(), Error> {
    let mut header = [0u8; HEADER_LEN];
    if read_full(input, &mut header)? < HEADER_LEN || !header.starts_with(MAGIC) {
        return Err(Error::BadArchive("not an encrypted Superhet backup".into()));
    }
    let word = |at: usize| {
        u32::from_be_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
    };
    let (m, t, p) = (word(8), word(12), word(16));
    if !(8..=M_KIB_MAX).contains(&m)
        || !(1..=T_COST_MAX).contains(&t)
        || !(1..=P_COST_MAX).contains(&p)
    {
        return Err(Error::BadArchive("unsupported key derivation cost".into()));
    }
    let salt = &header[20..20 + SALT_LEN];
    let mut prefix = [0u8; PREFIX_LEN];
    prefix.copy_from_slice(&header[20 + SALT_LEN..]);
    let key = aead_key(&derive(passphrase, salt, m, t, p)?)?;

    let mut cur = vec![0u8; CHUNK + TAG];
    let mut next = vec![0u8; CHUNK + TAG];
    let mut cur_len = read_full(input, &mut cur)?;
    let mut counter: u32 = 0;
    let mut written: u64 = 0;
    loop {
        let next_len = if cur_len == CHUNK + TAG {
            read_full(input, &mut next)?
        } else {
            0
        };
        let last = next_len == 0;
        if cur_len < TAG {
            return Err(Error::WrongPassphrase);
        }
        let plain = key
            .open_in_place(
                nonce(&prefix, counter, last),
                Aad::from(&header),
                &mut cur[..cur_len],
            )
            .map_err(|_| Error::WrongPassphrase)?;
        written += plain.len() as u64;
        if written > max_plain {
            return Err(Error::BadArchive(
                "the decrypted backup is too large".into(),
            ));
        }
        output.write_all(plain)?;
        if last {
            return Ok(());
        }
        counter = counter.checked_add(1).ok_or(Error::WrongPassphrase)?;
        std::mem::swap(&mut cur, &mut next);
        cur_len = next_len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seal(data: &[u8], pass: &str) -> Vec<u8> {
        let mut out = Vec::new();
        encrypt(&mut &data[..], &mut out, pass).expect("encrypt");
        out
    }

    fn open(data: &[u8], pass: &str) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        decrypt(&mut &data[..], &mut out, pass, 1 << 30).map(|()| out)
    }

    #[test]
    fn round_trips_around_chunk_boundaries() {
        for len in [0, 1, CHUNK - 1, CHUNK, CHUNK + 1, 2 * CHUNK, 2 * CHUNK + 17] {
            let data: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let sealed = seal(&data, "correct horse");
            assert_eq!(
                sealed.len(),
                HEADER_LEN + len + TAG * len.div_ceil(CHUNK).max(1),
                "len {len}"
            );
            assert_eq!(
                open(&sealed, "correct horse").expect("decrypt"),
                data,
                "len {len}"
            );
        }
    }

    #[test]
    fn wrong_passphrase_is_refused() {
        let sealed = seal(b"secret payload", "correct horse");
        assert!(matches!(
            open(&sealed, "battery staple"),
            Err(Error::WrongPassphrase)
        ));
    }

    #[test]
    fn tampering_header_or_body_is_refused() {
        let sealed = seal(&vec![7u8; CHUNK * 2 + 5], "correct horse");
        for at in [
            HEADER_LEN - 1,
            HEADER_LEN,
            HEADER_LEN + CHUNK + TAG + 3,
            sealed.len() - 1,
        ] {
            let mut bad = sealed.clone();
            bad[at] ^= 1;
            assert!(
                matches!(open(&bad, "correct horse"), Err(Error::WrongPassphrase)),
                "at {at}"
            );
        }
    }

    #[test]
    fn truncation_and_chunk_reordering_are_refused() {
        let sealed = seal(&vec![7u8; CHUNK * 3], "correct horse");
        let body = HEADER_LEN..;
        // Drop the (empty) last chunk: the new tail was not sealed as "last".
        assert!(open(&sealed[..sealed.len() - TAG], "correct horse").is_err());
        let one_chunk = HEADER_LEN + CHUNK + TAG;
        assert!(open(&sealed[..one_chunk], "correct horse").is_err());
        // Swap the first two chunks.
        let mut swapped = sealed.clone();
        let (a, b) = swapped[body].split_at_mut(CHUNK + TAG);
        a.swap_with_slice(&mut b[..CHUNK + TAG]);
        assert!(open(&swapped, "correct horse").is_err());
    }

    #[test]
    fn hostile_cost_parameters_and_size_limits() {
        let mut sealed = seal(b"x", "correct horse");
        sealed[8..12].copy_from_slice(&(4_000_000u32).to_be_bytes());
        assert!(matches!(
            open(&sealed, "correct horse"),
            Err(Error::BadArchive(_))
        ));
        let sealed = seal(&vec![1u8; 1000], "correct horse");
        let mut out = Vec::new();
        let err = decrypt(&mut &sealed[..], &mut out, "correct horse", 10).expect_err("too large");
        assert!(matches!(err, Error::BadArchive(_)));
    }

    #[test]
    fn short_passphrases_are_rejected_and_magic_is_detected() {
        let mut out = Vec::new();
        assert!(matches!(
            encrypt(&mut &b"x"[..], &mut out, "short"),
            Err(Error::Invalid(_))
        ));
        assert!(is_encrypted(&seal(b"x", "correct horse")));
        assert!(!is_encrypted(b"PK\x03\x04"));
    }
}
