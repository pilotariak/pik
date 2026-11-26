// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Ids of slots, members, guests and players. Ids appear in URLs, and a guessable one would tell
//! that a slot exists: every id holds enough random bits to be impossible to guess.
//!
//! A slot id is a UUID v7 (RFC 9562) in its canonical form, lowercase and hyphenated, without a
//! prefix: the creation time in milliseconds, then 74 random bits. The id of a newer slot sorts
//! after the older ones, as text too, so new rows land at the end of the index of the `slots`
//! primary key. Anyone who sees a slot id, such as in the slot URL shared with the group, learns
//! when the slot was created, to the millisecond.
//!
//! A member, guest or player id is a type prefix and 128 random bits in lowercase base32 (`m_`
//! and 26 characters), never sequential.

use core::fmt;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::random::{self, RandomError};

/// RFC 4648 base32 alphabet, lowercase, without padding.
const BASE32: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";

/// Characters of 128 bits in base32: 26 × 5 = 130 bits.
const ID_CHARS: usize = 26;

/// Lowercase hexadecimal digits.
const HEX: &[u8; 16] = b"0123456789abcdef";

/// Characters of a canonical UUID: 32 hexadecimal digits in groups of 8-4-4-4-12, and 4 hyphens.
const UUID_CHARS: usize = 36;

/// Bytes of a UUID that start a group of the canonical form, after a hyphen.
const UUID_GROUP_STARTS: [usize; 4] = [4, 6, 8, 10];

/// Positions of the hyphens in the canonical form.
const UUID_HYPHENS: [usize; 4] = [8, 13, 18, 23];

/// Position of the version digit in the canonical form: `7` for a UUID v7.
const UUID_VERSION: usize = 14;

/// Position of the digit whose two high bits are the variant, `10` for RFC 9562: `8` to `b`.
const UUID_VARIANT: usize = 19;

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

/// The canonical form of a UUID v7 (RFC 9562, section 5.7): `millis` in its first 48 bits, the
/// version and variant bits, and 74 of the 80 bits of `random`.
///
/// `millis` is a Unix time in milliseconds below 2^48, which every [`Timestamp`] after 1970 is.
fn uuid_v7(millis: u64, random: [u8; 10]) -> String {
    let mut bytes = [0; 16];
    // unix_ts_ms: the 48 low bits of `millis`, big-endian.
    bytes[..6].copy_from_slice(&millis.to_be_bytes()[2..]);
    bytes[6..].copy_from_slice(&random);
    // ver, 0b0111, then the 12 bits of rand_a.
    bytes[6] = 0x70 | (bytes[6] & 0x0F);
    // var, 0b10, then the 62 bits of rand_b.
    bytes[8] = 0x80 | (bytes[8] & 0x3F);

    let mut out = String::with_capacity(UUID_CHARS);
    for (index, &byte) in bytes.iter().enumerate() {
        if UUID_GROUP_STARTS.contains(&index) {
            out.push('-');
        }
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0F)]));
    }
    out
}

/// Whether `s` is the canonical form of a UUID v7 with the RFC 9562 variant: 36 characters,
/// lowercase hexadecimal digits and hyphens at their places.
fn is_uuid_v7(s: &str) -> bool {
    s.len() == UUID_CHARS
        && s.bytes().enumerate().all(|(index, byte)| match index {
            UUID_VERSION => byte == b'7',
            UUID_VARIANT => matches!(byte, b'8' | b'9' | b'a' | b'b'),
            _ if UUID_HYPHENS.contains(&index) => byte == b'-',
            _ => HEX.contains(&byte),
        })
}

/// An id type: a string, written as is in D1, URLs and bodies.
macro_rules! id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub(crate) struct $name(String);

        impl $name {
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

/// Ids of a type prefix and 128 random bits in base32.
macro_rules! prefixed {
    ($($name:ident => $prefix:literal),*) => {$(
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
        }
    )*};
}

/// Prefixed ids read from a path or a body. A player id never is: it comes from a token.
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
    /// `slots.id`: a UUID v7 in its canonical form.
    SlotId
);
id!(
    /// `members.id`.
    MemberId
);
id!(
    /// `guests.id`.
    GuestId
);
id!(
    /// `players.id`: one per device (kancha ADR-0002).
    PlayerId
);
prefixed!(MemberId => "m_", GuestId => "g_", PlayerId => "p_");
parse!(MemberId, GuestId);

impl SlotId {
    /// A new id for a slot created at `now`, the instant of the request: `now` in milliseconds,
    /// then 74 random bits.
    ///
    /// `domain` reads no clock: the route passes the instant it took from the Worker. An instant
    /// before 1970 counts as 1970, which the Worker's clock never reads.
    pub(crate) fn generate(now: Timestamp) -> Result<Self, RandomError> {
        let millis = u64::try_from(now.as_millisecond()).unwrap_or(0);
        Ok(Self(uuid_v7(millis, random::bytes::<10>()?)))
    }

    /// An id from a path or from `idempotency_keys`. `None` unless it is the canonical form of a
    /// UUID v7: an `s_` id, an uppercase or unhyphenated UUID and any other version are answered
    /// as an id that does not exist.
    pub(crate) fn parse(s: &str) -> Option<Self> {
        is_uuid_v7(s).then(|| Self(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use jiff::Timestamp;

    use super::{GuestId, MemberId, PlayerId, SlotId, base32, uuid_v7};

    /// The instant of the UUID v7 example of RFC 9562, appendix A.6: 2022-02-22T19:22:22Z.
    const RFC_MILLIS: i64 = 0x017F_22E2_79B0;

    /// The UUID v7 of RFC 9562, appendix A.6, in lowercase.
    const RFC_UUID: &str = "017f22e2-79b0-7cc3-98c4-dc0c0c07398f";

    fn instant(millis: i64) -> Timestamp {
        Timestamp::from_millisecond(millis).expect("should be an instant jiff represents")
    }

    /// The 48-bit timestamp of a canonical UUID: its first 12 hexadecimal digits.
    fn millis_of(id: &str) -> i64 {
        let digits: String = id[..13].chars().filter(|&c| c != '-').collect();
        i64::from_str_radix(&digits, 16).expect("should be 12 hexadecimal digits")
    }

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
    fn uuid_v7_matches_the_example_of_rfc_9562() {
        let random = [0x0C, 0xC3, 0x18, 0xC4, 0xDC, 0x0C, 0x0C, 0x07, 0x39, 0x8F];

        let uuid = uuid_v7(0x017F_22E2_79B0, random);

        assert_eq!(uuid, RFC_UUID, "the UUID v7 of RFC 9562, appendix A.6");
    }

    #[test]
    fn uuid_v7_keeps_74_random_bits_and_sets_the_version_and_variant() {
        for (random, expected) in [
            ([0x00; 10], "00000000-0000-7000-8000-000000000000"),
            ([0xFF; 10], "00000000-0000-7fff-bfff-ffffffffffff"),
        ] {
            assert_eq!(
                uuid_v7(0, random),
                expected,
                "version 7 and variant 0b10 over the random bits {random:?}"
            );
        }
    }

    #[test]
    fn uuid_v7_holds_every_instant_jiff_represents_after_1970() {
        let latest = u64::try_from(Timestamp::MAX.as_millisecond()).expect("should be after 1970");

        let uuid = uuid_v7(latest, [0; 10]);

        assert_eq!(
            millis_of(&uuid),
            Timestamp::MAX.as_millisecond(),
            "the latest instant of jiff, in 9999, fits in 48 bits"
        );
    }

    #[test]
    fn a_slot_id_is_a_uuid_v7_of_its_instant() {
        let now = instant(RFC_MILLIS);

        let slot = SlotId::generate(now).expect("should draw a slot id");

        assert_eq!(slot.as_str().len(), 36, "a canonical UUID");
        assert_eq!(
            millis_of(slot.as_str()),
            RFC_MILLIS,
            "the first 48 bits are the instant in milliseconds"
        );
        assert_eq!(&slot.as_str()[14..15], "7", "version 7");
        assert!(
            matches!(&slot.as_str()[19..20], "8" | "9" | "a" | "b"),
            "the variant of RFC 9562 in {slot}"
        );
        assert_eq!(
            SlotId::parse(slot.as_str()),
            Some(slot.clone()),
            "a generated id parses back"
        );
    }

    #[test]
    fn two_slot_ids_of_the_same_millisecond_differ() {
        let now = instant(RFC_MILLIS);

        let first = SlotId::generate(now).expect("should draw a slot id");
        let second = SlotId::generate(now).expect("should draw a slot id");

        assert_ne!(first, second, "74 random bits tell them apart");
    }

    #[test]
    fn a_slot_id_of_a_later_instant_sorts_after() {
        for (earlier, later) in [
            (RFC_MILLIS, RFC_MILLIS + 1),
            (0x09, 0x0A),
            (0x0F, 0x10),
            (0, RFC_MILLIS),
        ] {
            let first = SlotId::generate(instant(earlier)).expect("should draw a slot id");
            let second = SlotId::generate(instant(later)).expect("should draw a slot id");

            assert!(
                first.as_str() < second.as_str(),
                "the id of {later} ms sorts after the one of {earlier} ms, as text"
            );
        }
    }

    #[test]
    fn a_slot_id_before_1970_counts_as_1970() {
        let slot = SlotId::generate(instant(-1)).expect("should draw a slot id");

        assert_eq!(millis_of(slot.as_str()), 0, "the timestamp of 1970-01-01");
        assert!(
            SlotId::parse(slot.as_str()).is_some(),
            "still a UUID v7: {slot}"
        );
    }

    #[test]
    fn slot_id_parse_rejects_every_other_form() {
        assert!(SlotId::parse(RFC_UUID).is_some(), "the canonical form");
        let mut invalid = vec![
            "s_abcdefghijklmnopqrstuvwxyz".to_owned(),
            RFC_UUID.to_uppercase(),
            RFC_UUID.replace('-', ""),
            format!("{{{RFC_UUID}}}"),
            format!("urn:uuid:{RFC_UUID}"),
            format!("{RFC_UUID}0"),
            RFC_UUID[..35].to_owned(),
            "017f22e2-79b0-4cc3-98c4-dc0c0c07398f".to_owned(),
            "017f22e2-79b0-8cc3-98c4-dc0c0c07398f".to_owned(),
            "017f22e-279b0-7cc3-98c4-dc0c0c07398f".to_owned(),
            "017f22e2-79b0-7cc3-98c4-dc0c0c07398g".to_owned(),
            "017f22e2-79b0-7cc3-98c4-dc0c0c0739é".to_owned(),
            "me".to_owned(),
            String::new(),
        ];
        for variant in "01234567cdef".chars() {
            invalid.push(format!("{}{variant}{}", &RFC_UUID[..19], &RFC_UUID[20..]));
        }
        for id in invalid {
            assert_eq!(SlotId::parse(&id), None, "{id:?} is not a slot id");
        }
    }

    #[test]
    fn prefixed_ids_have_their_prefix_and_128_bits() {
        for (id, prefix) in [
            (MemberId::generate().expect("should draw").to_string(), "m_"),
            (GuestId::generate().expect("should draw").to_string(), "g_"),
            (PlayerId::generate().expect("should draw").to_string(), "p_"),
        ] {
            assert!(id.starts_with(prefix), "{id} starts with {prefix}");
            assert_eq!(id.len(), 28, "{id}: the prefix and 26 base32 characters");
        }
        let member = MemberId::generate().expect("should draw a member id");
        assert_eq!(
            MemberId::parse(member.as_str()),
            Some(member.clone()),
            "a generated id parses back"
        );
        assert_ne!(
            MemberId::generate().expect("should draw a member id"),
            member,
            "two ids differ"
        );
    }

    #[test]
    fn prefixed_id_parse_rejects_what_cannot_be_an_id() {
        let valid = "m_abcdefghijklmnopqrstuvwxyz";
        assert!(MemberId::parse(valid).is_some(), "26 base32 characters");
        for invalid in [
            "g_abcdefghijklmnopqrstuvwxyz",
            "s_abcdefghijklmnopqrstuvwxyz",
            "m_abcdefghijklmnopqrstuvwxy",
            "m_abcdefghijklmnopqrstuvwxyz2",
            "m_ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            "m_abcdefghijklmnopqrstuvwxy1",
            RFC_UUID,
            "me",
            "",
        ] {
            assert_eq!(
                MemberId::parse(invalid),
                None,
                "{invalid:?} is not a member id"
            );
        }
    }
}
