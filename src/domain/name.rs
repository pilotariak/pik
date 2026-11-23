// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Display names of members and guests, and the key that makes a name unique in a slot.

use core::fmt;

use icu_normalizer::ComposingNormalizerBorrowed;

/// Longest display name, in characters.
pub(crate) const MAX_NAME_CHARS: usize = 40;

/// A first name or a nickname, as shown, and its key.
///
/// The name is trimmed, its runs of white space become one space, and it is in Unicode NFC, so
/// that "Peio" typed on two keyboards is one name. The key (`members.name_key`) is the
/// lowercase of that name: "PEIO" and "peio" take the same place in a slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DisplayName {
    text: String,
    key: String,
}

/// A name that cannot be a display name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NameError {
    Empty,
    TooLong,
    ControlCharacter,
}

impl fmt::Display for NameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("a name cannot be empty"),
            Self::TooLong => write!(f, "a name has at most {MAX_NAME_CHARS} characters"),
            Self::ControlCharacter => f.write_str("a name cannot hold control characters"),
        }
    }
}

impl std::error::Error for NameError {}

/// NFC form of `s`.
fn nfc(s: &str) -> String {
    ComposingNormalizerBorrowed::new_nfc()
        .normalize(s)
        .into_owned()
}

impl DisplayName {
    pub(crate) fn parse(input: &str) -> Result<Self, NameError> {
        // Tabs and line breaks are white space, collapsed first: any control character left is
        // rejected.
        let text = nfc(&input.split_whitespace().collect::<Vec<_>>().join(" "));
        if text.chars().any(char::is_control) {
            return Err(NameError::ControlCharacter);
        }
        if text.is_empty() {
            return Err(NameError::Empty);
        }
        if text.chars().count() > MAX_NAME_CHARS {
            return Err(NameError::TooLong);
        }
        // Lowercasing can leave a string that is no longer in NFC: normalize again.
        let key = nfc(&text.to_lowercase());
        Ok(Self { text, key })
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    pub(crate) fn key(&self) -> &str {
        &self.key
    }
}

#[cfg(test)]
mod tests {
    use super::{DisplayName, MAX_NAME_CHARS, NameError};

    #[test]
    fn white_space_is_trimmed_and_collapsed() {
        let name = DisplayName::parse("  Jean   Pierre \t").expect("should accept the name");
        assert_eq!(name.text(), "Jean Pierre", "one space between words");
        assert_eq!(name.key(), "jean pierre", "the key is lowercase");
    }

    #[test]
    fn composed_and_decomposed_forms_share_one_key() {
        // "Mañex" with a precomposed ñ, then with n and a combining tilde.
        let composed = DisplayName::parse("Ma\u{f1}ex").expect("should accept the name");
        let decomposed = DisplayName::parse("Man\u{303}ex").expect("should accept the name");
        assert_eq!(composed, decomposed, "NFC makes both forms equal");
        assert_eq!(
            DisplayName::parse("MAÑEX")
                .expect("should accept the name")
                .key(),
            composed.key(),
            "case does not matter for the key"
        );
    }

    #[test]
    fn empty_long_or_control_names_are_rejected() {
        assert_eq!(DisplayName::parse(" \t "), Err(NameError::Empty), "blank");
        assert_eq!(
            DisplayName::parse(&"é".repeat(MAX_NAME_CHARS + 1)),
            Err(NameError::TooLong),
            "41 characters"
        );
        assert!(
            DisplayName::parse(&"é".repeat(MAX_NAME_CHARS)).is_ok(),
            "40 characters, counted as characters and not bytes"
        );
        assert_eq!(
            DisplayName::parse("Peio\u{0}"),
            Err(NameError::ControlCharacter),
            "NUL"
        );
    }
}
