// SPDX-FileCopyrightText: Copyright (C) Nicolas Lamirault <nicolas.lamirault@gmail.com>
// SPDX-License-Identifier: Apache-2.0

//! Random bytes for ids, invite codes and tokens.
//!
//! The only source is the platform CSPRNG through getrandom: `crypto.getRandomValues` (Web
//! Crypto) in the Worker, the OS on the host. Nothing here is seeded or predictable.

use core::fmt;

/// The CSPRNG failed: the Worker answers 500 rather than issue a weak value.
#[derive(Debug)]
pub(crate) struct RandomError(getrandom::Error);

impl fmt::Display for RandomError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the CSPRNG failed: {}", self.0)
    }
}

impl std::error::Error for RandomError {}

/// `N` bytes from the CSPRNG.
pub(crate) fn bytes<const N: usize>() -> Result<[u8; N], RandomError> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(RandomError)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::bytes;

    #[test]
    fn two_draws_differ() {
        let first = bytes::<32>().expect("should draw 32 bytes");
        let second = bytes::<32>().expect("should draw 32 bytes");
        assert_ne!(first, second, "two 256-bit draws should never be equal");
    }
}
