// Copyright (c) 2026 Felix Kahle
// SPDX-License-Identifier: MIT
//
// This file is part of memguard and is distributed under the MIT License; see the LICENSE file at the root of
// the repository for the full text.

//! The memory limit a process tree must stay within.

use std::str::FromStr;

use bytesize::ByteSize;

use crate::Error;

/// A resident-memory limit for a whole process tree; never zero.
///
/// A literal limit is written as a constant, so a zero limit fails the build:
///
/// ```
/// use memguard::{ByteSize, MemoryLimit};
///
/// const LIMIT: MemoryLimit = MemoryLimit::new(ByteSize::gb(10));
/// assert!(LIMIT.is_exceeded_by(ByteSize::gb(11)));
/// ```
///
/// A limit read from the outside is parsed, and must name its unit:
///
/// ```
/// use memguard::{ByteSize, MemoryLimit};
///
/// let limit: MemoryLimit = "1.5GiB".parse()?;
/// assert_eq!(limit.size(), ByteSize::mib(1536));
/// assert!("10".parse::<MemoryLimit>().is_err());
/// # Ok::<(), memguard::Error>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MemoryLimit(ByteSize);

impl MemoryLimit {
    /// Creates a limit of `size`.
    ///
    /// # Panics
    ///
    /// Panics if `size` is zero; in a `const` context that is a compile error.
    #[must_use]
    #[track_caller]
    pub const fn new(size: ByteSize) -> Self {
        assert!(size.as_u64() != 0, "MemoryLimit must be greater than zero");
        Self(size)
    }

    /// The limit as a byte size; never zero.
    #[must_use]
    pub const fn size(self) -> ByteSize {
        self.0
    }

    /// Whether a tree using `used` resident bytes is over the limit; using exactly the limit is not.
    #[must_use]
    pub fn is_exceeded_by(self, used: ByteSize) -> bool {
        used > self.0
    }
}

impl FromStr for MemoryLimit {
    type Err = Error;

    /// Parses a byte size with a unit, such as `10G`, `512M`, `10GB` or `1.5GiB`.
    ///
    /// `G`, `M` and `K` are decimal units (`10G` is 10 000 000 000 bytes); `Gi`, `GiB` and friends are binary.
    ///
    /// # Errors
    ///
    /// Returns [`Error::LimitWithoutUnit`] for a bare number, [`Error::LimitUnparsable`] for anything that is not a
    /// byte size, and [`Error::LimitZero`] for a size of zero.
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let trimmed = input.trim();
        if trimmed.ends_with(|c: char| c.is_ascii_digit() || c == '.') {
            return Err(Error::LimitWithoutUnit {
                input: input.to_owned(),
            });
        }
        let size = trimmed
            .parse::<ByteSize>()
            .map_err(|reason| Error::LimitUnparsable {
                input: input.to_owned(),
                reason,
            })?;
        if size.as_u64() == 0 {
            return Err(Error::LimitZero {
                input: input.to_owned(),
            });
        }
        Ok(Self(size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Units parse the way the README promises.
    #[test]
    fn parses_units() {
        let parse = |s: &str| s.parse::<MemoryLimit>().map(MemoryLimit::size).ok();
        assert_eq!(parse("10G"), Some(ByteSize::gb(10)));
        assert_eq!(parse("10GB"), Some(ByteSize::gb(10)));
        assert_eq!(parse("512M"), Some(ByteSize::mb(512)));
        assert_eq!(parse("1.5GiB"), Some(ByteSize::mib(1536)));
    }

    /// Bare numbers, zero and garbage are rejected with the matching error.
    #[test]
    fn rejects_bad_limits() {
        assert!(matches!(
            "10".parse::<MemoryLimit>(),
            Err(Error::LimitWithoutUnit { .. })
        ));
        assert!(matches!(
            "0G".parse::<MemoryLimit>(),
            Err(Error::LimitZero { .. })
        ));
        assert!(matches!(
            "ten gigs".parse::<MemoryLimit>(),
            Err(Error::LimitUnparsable { .. })
        ));
    }

    /// The type's name appears in its errors and its `Debug` output.
    #[test]
    fn names_itself() {
        let error = "0G".parse::<MemoryLimit>().unwrap_err();
        assert!(error.to_string().contains("MemoryLimit"));
        assert!(format!("{:?}", MemoryLimit::new(ByteSize::gb(1))).contains("MemoryLimit"));
    }

    /// Using exactly the limit is allowed; one byte more is not.
    #[test]
    fn limit_is_inclusive() {
        let limit = MemoryLimit::new(ByteSize::b(100));
        assert!(!limit.is_exceeded_by(ByteSize::b(100)));
        assert!(limit.is_exceeded_by(ByteSize::b(101)));
    }

    /// The const constructor rejects zero at run time too.
    #[test]
    #[should_panic(expected = "MemoryLimit must be greater than zero")]
    fn new_rejects_zero() {
        let _ = MemoryLimit::new(ByteSize::b(0));
    }
}
