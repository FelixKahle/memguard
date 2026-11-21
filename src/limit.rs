// Copyright (c) 2026 Felix Kahle
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! The limits a guarded command must stay within: resident memory and wall-clock time.

use std::str::FromStr;
use std::time::Duration;

use bytesize::ByteSize;

use crate::Error;

/// A resident-memory limit, for a whole process tree or for each of its processes; never zero.
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

/// A wall-clock time limit for a guarded command; never zero.
///
/// A literal limit is written as a constant, so a zero limit fails the build:
///
/// ```
/// use std::time::Duration;
///
/// use memguard::TimeLimit;
///
/// const LIMIT: TimeLimit = TimeLimit::new(Duration::from_secs(15 * 60));
/// assert!(LIMIT.is_exceeded_by(Duration::from_secs(16 * 60)));
/// ```
///
/// A limit read from the outside is parsed, and must name its unit:
///
/// ```
/// use std::time::Duration;
///
/// use memguard::TimeLimit;
///
/// let limit: TimeLimit = "1m 30s".parse()?;
/// assert_eq!(limit.duration(), Duration::from_secs(90));
/// assert!("90".parse::<TimeLimit>().is_err());
/// # Ok::<(), memguard::Error>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimeLimit(Duration);

impl TimeLimit {
    /// Creates a limit of `duration`.
    ///
    /// # Panics
    ///
    /// Panics if `duration` is zero; in a `const` context that is a compile error.
    #[must_use]
    #[track_caller]
    pub const fn new(duration: Duration) -> Self {
        assert!(!duration.is_zero(), "TimeLimit must be greater than zero");
        Self(duration)
    }

    /// The limit as a duration; never zero.
    #[must_use]
    pub const fn duration(self) -> Duration {
        self.0
    }

    /// Whether a command that has run for `elapsed` is over the limit; running for exactly the limit is not.
    #[must_use]
    pub fn is_exceeded_by(self, elapsed: Duration) -> bool {
        elapsed > self.0
    }
}

impl FromStr for TimeLimit {
    type Err = Error;

    /// Parses a duration with a unit, such as `90s`, `15m`, `1h 30m` or `500ms`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::TimeLimitUnparsable`] for anything that is not a duration with a unit, and
    /// [`Error::TimeLimitZero`] for a duration of zero.
    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let duration = humantime::parse_duration(input.trim()).map_err(|source| {
            Error::TimeLimitUnparsable {
                input: input.to_owned(),
                source,
            }
        })?;
        if duration.is_zero() {
            return Err(Error::TimeLimitZero {
                input: input.to_owned(),
            });
        }
        Ok(Self(duration))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Time units parse the way the README promises.
    #[test]
    fn parses_time_units() {
        let parse = |s: &str| s.parse::<TimeLimit>().map(TimeLimit::duration).ok();
        assert_eq!(parse("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse("90s"), Some(Duration::from_secs(90)));
        assert_eq!(parse("15m"), Some(Duration::from_secs(900)));
        assert_eq!(parse("1h 30m"), Some(Duration::from_secs(5400)));
    }

    /// Bare numbers, zero and garbage are rejected with the matching error, which names the type.
    #[test]
    fn rejects_bad_time_limits() {
        assert!(matches!(
            "90".parse::<TimeLimit>(),
            Err(Error::TimeLimitUnparsable { .. })
        ));
        assert!(matches!(
            "soon".parse::<TimeLimit>(),
            Err(Error::TimeLimitUnparsable { .. })
        ));
        let zero = "0s".parse::<TimeLimit>().unwrap_err();
        assert!(matches!(zero, Error::TimeLimitZero { .. }));
        assert!(zero.to_string().contains("TimeLimit"));
        assert!(format!("{:?}", TimeLimit::new(Duration::from_secs(1))).contains("TimeLimit"));
    }

    /// Running for exactly the limit is allowed; one nanosecond more is not.
    #[test]
    fn time_limit_is_inclusive() {
        let limit = TimeLimit::new(Duration::from_secs(1));
        assert!(!limit.is_exceeded_by(Duration::from_secs(1)));
        assert!(limit.is_exceeded_by(Duration::from_nanos(1_000_000_001)));
    }

    /// The const constructor rejects zero at run time too.
    #[test]
    #[should_panic(expected = "TimeLimit must be greater than zero")]
    fn time_limit_new_rejects_zero() {
        let _ = TimeLimit::new(Duration::ZERO);
    }

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
