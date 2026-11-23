// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Ids of slots, members, guests and players.
//!
//! An id is a type prefix and 128 random bits in lowercase base32 (`s_` and 26 characters),
//! never sequential: ids appear in URLs, and a guessable one would tell that a slot exists.

use core::fmt;

use serde::{Deserialize, Serialize};

use super::random::{self, RandomError};

/// RFC 4648 base32 alphabet, lowercase, without padding.
const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// Characters of 128 bits in base32: 26 × 5 = 130 bits.
const ID_CHARS: usize = 26;

/// Encodes bytes in lowercase base32 without padding.
pub(crate) fn base32(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(5) * 8);
    let mut buffer: u32 = 0;
    let mut bits = 0;
    for &byte in bytes {
        buffer = (buffer << 8) | u32::from(byte);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(char::from(BASE32[((buffer >> bits) & 31) as usize]));
        }
    }
    if bits > 0 {
        out.push(char::from(BASE32[((buffer << (5 - bits)) & 31) as usize]));
    }
    out
}

/// Whether `s` is `len` characters of the base32 alphabet.
pub(crate) fn is_base32(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|byte| BASE32.contains(&byte))
}

macro_rules! id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub(crate) struct $name(String);

        impl $name {
            const PREFIX: &'static str = $prefix;

            /// A new random id.
            pub(crate) fn generate() -> Result<Self, RandomError> {
                Ok(Self(format!(
                    "{}{}",
                    Self::PREFIX,
                    base32(&random::bytes::<16>()?)
                )))
            }

            pub(crate) fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

/// Ids read from a path or a body. A player id never is: it comes from a token.
macro_rules! parse {
    ($($name:ident),*) => {$(
        impl $name {
            /// An id from a path or a body. `None` when it cannot be an id of this type: the
            /// caller answers as for an id that does not exist.
            pub(crate) fn parse(s: &str) -> Option<Self> {
                let rest = s.strip_prefix(Self::PREFIX)?;
                is_base32(rest, ID_CHARS).then(|| Self(s.to_owned()))
            }
        }
    )*};
}

id!(
    /// `slots.id`.
    SlotId,
    "s_"
);
id!(
    /// `members.id`.
    MemberId,
    "m_"
);
id!(
    /// `guests.id`.
    GuestId,
    "g_"
);
id!(
    /// `players.id`: one per device (kancha ADR-0002).
    PlayerId,
    "p_"
);
parse!(SlotId, MemberId, GuestId);

#[cfg(test)]
mod tests {
    use super::{GuestId, MemberId, PlayerId, SlotId, base32};

    #[test]
    fn base32_follows_rfc_4648_without_padding() {
        for (input, expected) in [
            (&b""[..], ""),
            (b"f", "my"),
            (b"fo", "mzxq"),
            (b"foo", "mzxw6"),
            (b"foob", "mzxw6yq"),
            (b"fooba", "mzxw6ytb"),
            (b"foobar", "mzxw6ytboi"),
        ] {
            assert_eq!(base32(input), expected, "base32 of {input:?}");
        }
    }

    #[test]
    fn generated_ids_have_their_prefix_and_128_bits() {
        let slot = SlotId::generate().expect("should draw a slot id");
        assert!(slot.as_str().starts_with("s_"), "slot ids start with s_");
        assert_eq!(slot.as_str().len(), 28, "prefix and 26 base32 characters");
        assert_eq!(
            SlotId::parse(slot.as_str()),
            Some(slot.clone()),
            "a generated id parses back"
        );
        assert_ne!(
            SlotId::generate().expect("should draw a slot id"),
            slot,
            "two ids differ"
        );
        for (id, prefix) in [
            (MemberId::generate().expect("should draw").to_string(), "m_"),
            (GuestId::generate().expect("should draw").to_string(), "g_"),
            (PlayerId::generate().expect("should draw").to_string(), "p_"),
        ] {
            assert!(id.starts_with(prefix), "{id} starts with {prefix}");
        }
    }

    #[test]
    fn parse_rejects_what_cannot_be_an_id() {
        let valid = "s_abcdefghijklmnopqrstuvwxyz";
        assert!(SlotId::parse(valid).is_some(), "26 base32 characters");
        for invalid in [
            "m_abcdefghijklmnopqrstuvwxyz",
            "s_abcdefghijklmnopqrstuvwxy",
            "s_abcdefghijklmnopqrstuvwxyz2",
            "s_ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            "s_abcdefghijklmnopqrstuvwxy1",
            "me",
            "",
        ] {
            assert_eq!(SlotId::parse(invalid), None, "{invalid:?} is not a slot id");
        }
    }
}
