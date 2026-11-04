//! Decrypts SecureCRT's `Password V2` values, which a person importing their own sessions
//! needs so they do not have to retype them.
//!
//! `02:<hex>` is AES-256-CBC with a key of SHA-256(passphrase) and an all-zero IV. `03:<hex>`
//! starts with a 16-byte salt, and bcrypt_pbkdf(passphrase, salt, 16 rounds) gives the key and
//! IV. Both decrypt to a little-endian length, the password, its SHA-256, then padding, and
//! that checksum is how a wrong passphrase is recognised. The passphrase is empty unless the
//! user set one in SecureCRT.

use aes::cipher::{block_padding::NoPadding, BlockDecryptMut, KeyIvInit};
use blowfish::Blowfish;
use sha2::{Digest, Sha256, Sha512};
use zeroize::Zeroizing;

type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

const BCRYPT_ROUNDS: u32 = 16;
const SALT_LEN: usize = 16;
const MAX_CIPHERTEXT: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecryptError {
    /// The value is intact but this passphrase does not open it.
    WrongPassphrase,
    /// Not a value this understands, such as the older `Password` format.
    Unsupported,
}

/// Decrypts one `Password V2` value.
pub fn decrypt_password_v2(
    encrypted: &str,
    passphrase: &str,
) -> Result<Zeroizing<String>, DecryptError> {
    let encrypted = encrypted.trim();
    let (salted, hex) = match encrypted.split_once(':') {
        Some(("02", hex)) => (false, hex),
        Some(("03", hex)) => (true, hex),
        Some(_) => return Err(DecryptError::Unsupported),
        None => (false, encrypted),
    };
    let data = decode_hex(hex).ok_or(DecryptError::Unsupported)?;
    if data.len() > MAX_CIPHERTEXT {
        return Err(DecryptError::Unsupported);
    }
    if salted {
        if data.len() <= SALT_LEN {
            return Err(DecryptError::Unsupported);
        }
        let (salt, ciphertext) = data.split_at(SALT_LEN);
        let mut derived = Zeroizing::new([0u8; 48]);
        bcrypt_pbkdf(passphrase.as_bytes(), salt, BCRYPT_ROUNDS, derived.as_mut());
        let (key, iv) = derived.split_at(32);
        open(ciphertext, key, iv)
    } else {
        let key = Zeroizing::new(<[u8; 32]>::from(Sha256::digest(passphrase.as_bytes())));
        open(&data, key.as_ref(), &[0u8; 16])
    }
}

/// An imported password waiting to be decrypted, and the login it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPassword<T> {
    pub owner: T,
    pub encrypted: String,
}

/// What came of decrypting a batch of passwords.
pub struct Decrypted<T> {
    pub opened: Vec<(T, Zeroizing<String>)>,
    /// These need a different passphrase.
    pub locked: Vec<PendingPassword<T>>,
    /// These are in a format that cannot be read, such as the older `Password` field.
    pub unreadable: usize,
}

/// Decrypts on every core, as a salted value takes a noticeable fraction of a second.
pub fn decrypt_all<T: Send>(pending: Vec<PendingPassword<T>>, passphrase: &str) -> Decrypted<T> {
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get());
    let size = pending.len().div_ceil(threads).max(1);
    let mut batches: Vec<Vec<PendingPassword<T>>> = Vec::new();
    let mut pending = pending.into_iter().peekable();
    while pending.peek().is_some() {
        batches.push(pending.by_ref().take(size).collect());
    }
    let results: Vec<Vec<_>> = std::thread::scope(|scope| {
        let workers: Vec<_> = batches
            .into_iter()
            .map(|batch| {
                scope.spawn(move || {
                    batch
                        .into_iter()
                        .map(|item| {
                            let result = decrypt_password_v2(&item.encrypted, passphrase);
                            (item, result)
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .map(|w| w.join().unwrap_or_default())
            .collect()
    });
    let mut outcome = Decrypted {
        opened: Vec::new(),
        locked: Vec::new(),
        unreadable: 0,
    };
    for (item, result) in results.into_iter().flatten() {
        match result {
            Ok(password) => outcome.opened.push((item.owner, password)),
            Err(DecryptError::WrongPassphrase) => outcome.locked.push(item),
            Err(DecryptError::Unsupported) => outcome.unreadable += 1,
        }
    }
    outcome
}

fn open(ciphertext: &[u8], key: &[u8], iv: &[u8]) -> Result<Zeroizing<String>, DecryptError> {
    if ciphertext.is_empty() || !ciphertext.len().is_multiple_of(16) {
        return Err(DecryptError::Unsupported);
    }
    let mut plain = Zeroizing::new(ciphertext.to_vec());
    Aes256CbcDec::new_from_slices(key, iv)
        .map_err(|_| DecryptError::Unsupported)?
        .decrypt_padded_mut::<NoPadding>(&mut plain)
        .map_err(|_| DecryptError::Unsupported)?;
    let length = plain
        .get(..4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
        .ok_or(DecryptError::WrongPassphrase)?;
    // Garbage from a wrong key usually fails here, and the checksum catches the rest.
    let end = length.checked_add(4).ok_or(DecryptError::WrongPassphrase)?;
    let checksum_end = end.checked_add(32).ok_or(DecryptError::WrongPassphrase)?;
    if checksum_end > plain.len() {
        return Err(DecryptError::WrongPassphrase);
    }
    let password = &plain[4..end];
    if Sha256::digest(password).as_slice() != &plain[end..checksum_end] {
        return Err(DecryptError::WrongPassphrase);
    }
    let text = std::str::from_utf8(password).map_err(|_| DecryptError::Unsupported)?;
    Ok(Zeroizing::new(text.to_string()))
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || !text.len().is_multiple_of(2) || !text.is_ascii() {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).ok())
        .collect()
}

/// OpenBSD's bcrypt_pbkdf. The usual crate refuses an empty passphrase, which is the normal
/// case here.
fn bcrypt_pbkdf(passphrase: &[u8], salt: &[u8], rounds: u32, output: &mut [u8]) {
    const HASH: usize = 32;
    let stride = output.len().div_ceil(HASH);
    let sha2pass = Zeroizing::new(<[u8; 64]>::from(Sha512::digest(passphrase)));
    for block in 1..=stride {
        let mut sha2salt: [u8; 64] = Sha512::new()
            .chain_update(salt)
            .chain_update((block as u32).to_be_bytes())
            .finalize()
            .into();
        let mut round = bcrypt_hash(&sha2pass, &sha2salt);
        let mut sum = round;
        for _ in 1..rounds {
            sha2salt = Sha512::digest(round).into();
            round = bcrypt_hash(&sha2pass, &sha2salt);
            for (total, byte) in sum.iter_mut().zip(round) {
                *total ^= byte;
            }
        }
        // The output takes one byte from each block in turn.
        for (i, byte) in sum.iter().enumerate() {
            if let Some(slot) = output.get_mut(i * stride + block - 1) {
                *slot = *byte;
            }
        }
    }
}

fn bcrypt_hash(sha2pass: &[u8; 64], sha2salt: &[u8; 64]) -> [u8; 32] {
    let mut state = Blowfish::bc_init_state();
    state.salted_expand_key(sha2salt, sha2pass);
    for _ in 0..64 {
        state.bc_expand_key(sha2salt);
        state.bc_expand_key(sha2pass);
    }
    let mut words = [0u32; 8];
    let (magic, _) = b"OxychromaticBlowfishSwatDynamite".as_chunks::<4>();
    for (word, bytes) in words.iter_mut().zip(magic) {
        *word = u32::from_be_bytes(*bytes);
    }
    for _ in 0..64 {
        let (pairs, _) = words.as_chunks_mut::<2>();
        for pair in pairs {
            *pair = state.bc_encrypt(*pair);
        }
    }
    let mut out = [0u8; 32];
    let (chunks, _) = out.as_chunks_mut::<4>();
    for (bytes, word) in chunks.iter_mut().zip(words) {
        *bytes = word.to_le_bytes();
    }
    out
}

/// Encrypts as SecureCRT does. Tests elsewhere in this crate use it to build fixtures.
#[cfg(test)]
pub(crate) fn encrypt_03(password: &str, passphrase: &str, salt: [u8; 16]) -> String {
    use aes::cipher::BlockEncryptMut;
    type Enc = cbc::Encryptor<aes::Aes256>;
    let mut derived = [0u8; 48];
    bcrypt_pbkdf(passphrase.as_bytes(), &salt, BCRYPT_ROUNDS, &mut derived);
    let mut plain = (password.len() as u32).to_le_bytes().to_vec();
    plain.extend_from_slice(password.as_bytes());
    plain.extend_from_slice(&Sha256::digest(password.as_bytes()));
    plain.resize(plain.len().next_multiple_of(16), 0);
    let length = plain.len();
    Enc::new_from_slices(&derived[..32], &derived[32..])
        .unwrap()
        .encrypt_padded_mut::<NoPadding>(&mut plain, length)
        .unwrap();
    let hex: String = salt
        .iter()
        .chain(&plain)
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("03:{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Made with the openssl command line, independently of this code.
    const OPENSSL_EMPTY: &str = "02:a946feca74f2916b52e36e1458fa1a6647b1af207ac92ee8b3273d4f6e54f8c85d5a0dcd7431cd38b8cab7f85133bddf";
    const OPENSSL_PHRASE: &str = "02:ada4e6e1709568d558ffc67a7cdd4d24b9e7187c7635b2550da50d6e6fb3acc72cd99789db9b703fd3e9daeb65dd161f";

    #[test]
    fn prefix_02_matches_an_independently_made_value() {
        assert_eq!(*decrypt_password_v2(OPENSSL_EMPTY, "").unwrap(), "hunter2");
        assert_eq!(
            *decrypt_password_v2(OPENSSL_PHRASE, "config phrase").unwrap(),
            "hunter2"
        );
        // Some versions leave off the prefix.
        let bare = OPENSSL_EMPTY.strip_prefix("02:").unwrap();
        assert_eq!(*decrypt_password_v2(bare, "").unwrap(), "hunter2");
    }

    #[test]
    fn a_wrong_passphrase_is_reported_as_one() {
        assert_eq!(
            decrypt_password_v2(OPENSSL_PHRASE, "").unwrap_err(),
            DecryptError::WrongPassphrase
        );
        let salted = encrypt_03("secret", "right", [3; 16]);
        assert_eq!(
            decrypt_password_v2(&salted, "wrong").unwrap_err(),
            DecryptError::WrongPassphrase
        );
    }

    #[test]
    fn prefix_03_works_with_and_without_a_passphrase() {
        for phrase in ["", "config phrase"] {
            let value = encrypt_03("pässword with spaces", phrase, [9; 16]);
            assert_eq!(
                *decrypt_password_v2(&value, phrase).unwrap(),
                "pässword with spaces"
            );
        }
    }

    #[test]
    fn this_kdf_agrees_with_the_published_crate() {
        let mut ours = [0u8; 48];
        let mut theirs = [0u8; 48];
        bcrypt_pbkdf(b"password", b"salt value", 4, &mut ours);
        bcrypt_pbkdf::bcrypt_pbkdf("password", b"salt value", 4, &mut theirs).unwrap();
        assert_eq!(ours, theirs);
    }

    #[test]
    fn a_batch_sorts_into_opened_locked_and_unreadable() {
        let pending = vec![
            PendingPassword {
                owner: 1,
                encrypted: OPENSSL_EMPTY.to_string(),
            },
            PendingPassword {
                owner: 2,
                encrypted: OPENSSL_PHRASE.to_string(),
            },
            PendingPassword {
                owner: 3,
                encrypted: "u0123abcd".to_string(),
            },
        ];
        let outcome = decrypt_all(pending, "");
        assert_eq!(outcome.opened.len(), 1);
        assert_eq!(outcome.opened[0].0, 1);
        assert_eq!(*outcome.opened[0].1, "hunter2");
        assert_eq!(outcome.locked.len(), 1);
        assert_eq!(outcome.locked[0].owner, 2);
        assert_eq!(outcome.unreadable, 1);
        assert_eq!(
            *decrypt_all(outcome.locked, "config phrase").opened[0].1,
            "hunter2"
        );
    }

    #[test]
    fn damaged_values_fail_cleanly() {
        for bad in [
            "", "02:", "03:", "02:zz", "02:abc", "03:00ff", "09:00ff", "u1234", "02:00",
        ] {
            assert!(decrypt_password_v2(bad, "").is_err(), "accepted {bad:?}");
        }
        // Every block length, so a short or lying length field cannot panic.
        for length in [16, 32, 48, 64, 256] {
            let value = format!("02:{}", "ff".repeat(length));
            assert!(decrypt_password_v2(&value, "").is_err());
        }
    }
}
