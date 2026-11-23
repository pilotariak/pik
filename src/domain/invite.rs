// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Invite code of a slot (kancha ADR-0002).
//!
//! Eight characters without 0, O, 1 or I, so that it reads aloud and types without mistakes. It
//! is only used to join: after that, calls use the slot id and the player token, so rotating it
//! does not affect members who already joined.

use core::fmt;

use serde::{Deserialize, Serialize};

use super::random::{self, RandomError};

/// 24 letters without O and I, and 8 digits without 0 and 1: 32 symbols, 5 bits each.
const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

const LEN: usize = 8;

/// `slots.invite_code`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct InviteCode(String);

impl InviteCode {
    /// A new random code: 40 bits. 256 is a multiple of 32, so the low 5 bits of each random
    /// byte pick a symbol without bias.
    pub(crate) fn generate() -> Result<Self, RandomError> {
        let code = random::bytes::<LEN>()?
            .iter()
            .map(|byte| char::from(ALPHABET[usize::from(byte & 31)]))
            .collect();
        Ok(Self(code))
    }

    /// A code typed or opened by a player, in any case. `None` when it cannot be a code: the
    /// caller answers as for a code that does not exist.
    pub(crate) fn parse(s: &str) -> Option<Self> {
        let code = s.to_ascii_uppercase();
        (code.len() == LEN && code.bytes().all(|byte| ALPHABET.contains(&byte)))
            .then_some(Self(code))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for InviteCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::InviteCode;

    #[test]
    fn generated_code_has_eight_unambiguous_characters() {
        for _ in 0..200 {
            let code = InviteCode::generate().expect("should draw a code");
            assert_eq!(code.as_str().len(), 8, "eight characters: {code}");
            assert!(
                !code.as_str().contains(['0', 'O', '1', 'I']),
                "no 0, O, 1 or I: {code}"
            );
            assert_eq!(
                InviteCode::parse(code.as_str()),
                Some(code.clone()),
                "a generated code parses back"
            );
        }
    }

    #[test]
    fn parse_accepts_any_case_and_rejects_ambiguous_characters() {
        assert_eq!(
            InviteCode::parse("abcd2345").map(|code| code.to_string()),
            Some("ABCD2345".to_owned()),
            "lowercase is read as uppercase"
        );
        for invalid in [
            "ABCD234",
            "ABCD23456",
            "ABCD234O",
            "ABCD2341",
            "ABCD234I",
            "ABCD-234",
        ] {
            assert_eq!(InviteCode::parse(invalid), None, "{invalid} is not a code");
        }
    }
}
