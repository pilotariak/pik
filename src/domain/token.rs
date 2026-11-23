// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Player tokens, recovery tokens and `Idempotency-Key` values: secrets that pik never stores
//! or logs in clear, only their SHA-256.

use core::fmt;

use sha2::{Digest as _, Sha256};

use super::ids::{base32, is_base32};
use super::random::{self, RandomError};

/// Characters of 256 bits in base32: 52 × 5 = 260 bits.
const TOKEN_CHARS: usize = 52;

/// SHA-256 of `bytes`.
pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// Lowercase hexadecimal of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 15)]));
    }
    out
}

/// A player token or a recovery token: 256 random bits in base32.
///
/// `Debug` hides the value, so that a token never reaches a log by accident.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Token(String);

impl Token {
    /// A new random token.
    pub(crate) fn generate() -> Result<Self, RandomError> {
        Ok(Self(base32(&random::bytes::<32>()?)))
    }

    /// A token sent by a client. `None` when it cannot be a token: the caller answers as for an
    /// unknown token, without hashing arbitrary input.
    pub(crate) fn parse(s: &str) -> Option<Self> {
        is_base32(s, TOKEN_CHARS).then(|| Self(s.to_owned()))
    }

    /// The value sent to the client, once.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }

    /// The only form stored: `players.token_hash` and `recoveries.token_hash`.
    pub(crate) fn hash(&self) -> TokenHash {
        TokenHash(sha256(self.0.as_bytes()))
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(<redacted>)")
    }
}

/// SHA-256 of a token, stored as a 32-byte BLOB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TokenHash([u8; 32]);

impl TokenHash {
    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Hexadecimal form, for `idempotency_keys.result_ref` of a recovery link.
    pub(crate) fn to_hex(self) -> String {
        hex(&self.0)
    }

    /// The hash of a `result_ref`.
    pub(crate) fn from_hex(s: &str) -> Option<Self> {
        if s.len() != 64 {
            return None;
        }
        let mut bytes = [0; 32];
        let (pairs, _) = s.as_bytes().as_chunks::<2>();
        for (byte, pair) in bytes.iter_mut().zip(pairs) {
            let pair = core::str::from_utf8(pair).ok()?;
            *byte = u8::from_str_radix(pair, 16).ok()?;
        }
        Some(Self(bytes))
    }
}

/// An `Idempotency-Key` header: a UUID v4 drawn by the client.
///
/// The key is a secret: a replay with it returns a fresh token. `idempotency_keys.key` holds
/// its SHA-256 in hexadecimal, never the key itself, and `Debug` hides it.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct IdempotencyKey(String);

impl IdempotencyKey {
    /// A UUID v4 in its 8-4-4-4-12 hexadecimal form, any case.
    pub(crate) fn parse(s: &str) -> Option<Self> {
        let key = s.to_ascii_lowercase();
        let bytes = key.as_bytes();
        let shape = bytes.len() == 36
            && bytes.iter().enumerate().all(|(i, byte)| match i {
                8 | 13 | 18 | 23 => *byte == b'-',
                _ => byte.is_ascii_hexdigit(),
            });
        let v4 = shape && bytes[14] == b'4' && matches!(bytes[19], b'8' | b'9' | b'a' | b'b');
        v4.then_some(Self(key))
    }

    /// The stored form.
    pub(crate) fn hash(&self) -> String {
        hex(&sha256(self.0.as_bytes()))
    }
}

impl fmt::Debug for IdempotencyKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("IdempotencyKey(<redacted>)")
    }
}

#[cfg(test)]
mod tests {
    use super::{IdempotencyKey, Token, TokenHash, hex, sha256};

    #[test]
    fn sha256_matches_the_fips_180_2_vector() {
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            "SHA-256 of abc"
        );
    }

    #[test]
    fn a_token_is_256_bits_and_hashes_to_its_sha_256() {
        let token = Token::generate().expect("should draw a token");
        assert_eq!(token.expose().len(), 52, "52 base32 characters");
        assert_eq!(
            Token::parse(token.expose()),
            Some(token.clone()),
            "a token parses back"
        );
        assert_eq!(
            token.hash().as_bytes(),
            &sha256(token.expose().as_bytes()),
            "the stored hash is the SHA-256 of the token"
        );
        assert_ne!(
            Token::generate().expect("should draw a token"),
            token,
            "two tokens differ"
        );
        assert_eq!(
            format!("{token:?}"),
            "Token(<redacted>)",
            "Debug never shows a token"
        );
    }

    #[test]
    fn token_parse_rejects_other_shapes() {
        for invalid in [
            "",
            "short",
            &"a".repeat(51),
            &"a".repeat(53),
            &"A".repeat(52),
        ] {
            assert_eq!(Token::parse(invalid), None, "{invalid:?} is not a token");
        }
    }

    #[test]
    fn token_hash_round_trips_through_hex() {
        let hash = Token::generate().expect("should draw a token").hash();
        assert_eq!(
            TokenHash::from_hex(&hash.to_hex()),
            Some(hash),
            "hex round trip"
        );
        assert_eq!(TokenHash::from_hex("zz"), None, "not 64 hex digits");
        assert_eq!(
            TokenHash::from_hex(&"g".repeat(64)),
            None,
            "not hexadecimal"
        );
    }

    #[test]
    fn idempotency_key_is_a_uuid_v4() {
        let key = IdempotencyKey::parse("3F2504E0-4F89-41D3-9A0C-0305E82C3301")
            .expect("should accept an uppercase UUID v4");
        assert_eq!(
            IdempotencyKey::parse("3f2504e0-4f89-41d3-9a0c-0305e82c3301"),
            Some(key.clone()),
            "the key is case-insensitive"
        );
        assert_eq!(key.hash().len(), 64, "the stored form is a SHA-256");
        assert_eq!(
            format!("{key:?}"),
            "IdempotencyKey(<redacted>)",
            "Debug never shows a key"
        );
        for invalid in [
            "3f2504e0-4f89-11d3-9a0c-0305e82c3301",
            "3f2504e0-4f89-41d3-7a0c-0305e82c3301",
            "3f2504e04f8941d39a0c0305e82c3301",
            "3f2504e0-4f89-41d3-9a0c-0305e82c330g",
            "",
        ] {
            assert_eq!(
                IdempotencyKey::parse(invalid),
                None,
                "{invalid:?} is not a UUID v4"
            );
        }
    }
}
