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

//! Which commands must run through `memguard run`.
//!
//! The rules are data. [`Rules`] lists the regexes of commands that must be guarded, the regexes of commands that are
//! exempt, and the reason given when a command is denied. [`Policy::compile`] checks and compiles them once;
//! [`Policy::decide`] is then a pure function of the command line.

use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

use crate::Error;

/// The token in [`Rules::reason`] that is replaced by the denied command.
pub const COMMAND_TOKEN: &str = "{command}";

/// A command that invokes `memguard run` is the guard itself, so it is always allowed.
static INVOKES_GUARD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(^|[\s;&|(])(\S*/)?memguard\s+run\b")
        .expect("the built-in memguard pattern is a valid regex")
});

/// The rules as written in a rules file.
///
/// A command is denied when it matches any `guarded` pattern, no `exempt` pattern, and does not invoke
/// `memguard run`. Patterns are regexes in multi-line mode, so `^` and `$` also match at line breaks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rules {
    /// Patterns of commands that must run through the guard; empty means nothing is ever denied.
    pub guarded: Vec<String>,
    /// Patterns of commands that are allowed even if they match `guarded`; absent in the file means none.
    #[serde(default)]
    pub exempt: Vec<String>,
    /// The explanation given with a denial; every [`COMMAND_TOKEN`] in it is replaced by the denied command.
    pub reason: String,
}

/// Compiled, valid [`Rules`].
#[derive(Debug, Clone)]
pub struct Policy {
    /// Compiled [`Rules::guarded`].
    guarded: Vec<Regex>,
    /// Compiled [`Rules::exempt`].
    exempt: Vec<Regex>,
    /// [`Rules::reason`], unsubstituted.
    reason: String,
}

/// A policy's verdict that a command must not run as written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denial {
    /// The explanation, with the command already substituted for [`COMMAND_TOKEN`].
    pub reason: String,
}

impl Policy {
    /// Compiles every pattern of `rules`.
    ///
    /// ```
    /// use memguard::policy::{Policy, Rules};
    ///
    /// let policy = Policy::compile(Rules {
    ///     guarded: vec![r"(^|[;&|]\s*)npm\s+test\b".to_owned()],
    ///     exempt: vec![],
    ///     reason: "Run `{command}` through memguard run.".to_owned(),
    /// })?;
    /// assert!(policy.decide("npm test").is_some());
    /// assert!(policy.decide("memguard run 4G npm test").is_none());
    /// # Ok::<(), memguard::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`Error::PatternInvalid`] naming the first pattern that is not a valid regex.
    pub fn compile(rules: Rules) -> Result<Self, Error> {
        Ok(Self {
            guarded: compile_all(rules.guarded)?,
            exempt: compile_all(rules.exempt)?,
            reason: rules.reason,
        })
    }

    /// Denies `command` if it matches a guarded pattern, no exempt pattern, and does not invoke `memguard run`.
    #[must_use]
    pub fn decide(&self, command: &str) -> Option<Denial> {
        let guarded = self.guarded.iter().any(|pattern| pattern.is_match(command));
        let exempt = INVOKES_GUARD.is_match(command)
            || self.exempt.iter().any(|pattern| pattern.is_match(command));
        (guarded && !exempt).then(|| Denial {
            reason: self.reason.replace(COMMAND_TOKEN, command),
        })
    }
}

/// Compiles each of `patterns` in multi-line mode.
///
/// # Errors
///
/// Returns [`Error::PatternInvalid`] naming the first pattern that is not a valid regex.
fn compile_all(patterns: Vec<String>) -> Result<Vec<Regex>, Error> {
    patterns
        .into_iter()
        .map(
            |pattern| match RegexBuilder::new(&pattern).multi_line(true).build() {
                Ok(regex) => Ok(regex),
                Err(source) => Err(Error::PatternInvalid { pattern, source }),
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// The example rules file shipped in the repository.
    const EXAMPLE: &str = include_str!("../examples/rules.toml");

    /// The policy built from the shipped example rules file.
    fn example() -> Policy {
        Policy::compile(toml::from_str(EXAMPLE).expect("the example rules parse"))
            .expect("the example compiles")
    }

    /// A policy from patterns written inline.
    fn policy(guarded: &[&str], exempt: &[&str], reason: &str) -> Policy {
        let owned = |patterns: &[&str]| patterns.iter().map(|p| (*p).to_owned()).collect();
        Policy::compile(Rules {
            guarded: owned(guarded),
            exempt: owned(exempt),
            reason: reason.to_owned(),
        })
        .expect("the test patterns compile")
    }

    /// Whether the example rules deny `command`.
    fn denied(command: &str) -> bool {
        example().decide(command).is_some()
    }

    /// The shipped rules deny cargo runs and hashed test binaries, on any line.
    #[test]
    fn example_denies_unguarded_runs() {
        assert!(denied("cargo test"));
        assert!(denied("RUSTFLAGS=-Cdebuginfo=0 cargo +nightly test"));
        assert!(denied("cargo build && cargo test"));
        assert!(denied("cargo nextest run"));
        assert!(denied("./target/debug/deps/foo-0123456789abcdef"));
        assert!(denied("cd crate\ncargo test --release"));
    }

    /// The shipped rules allow builds, mentions of cargo, unhashed binaries and guarded runs.
    #[test]
    fn example_allows_builds_and_guarded_runs() {
        assert!(!denied("cargo test --no-run"));
        assert!(!denied("cargo build"));
        assert!(!denied("echo cargo test"));
        assert!(!denied("target/release/memguard"));
        assert!(!denied("memguard run 10G cargo test"));
        assert!(!denied(
            "~/.cargo/bin/memguard run 10G cargo test -- --nocapture"
        ));
    }

    /// With no guarded patterns nothing is denied.
    #[test]
    fn empty_rules_allow_everything() {
        assert_eq!(policy(&[], &[], "no").decide("cargo test"), None);
    }

    /// An invalid pattern is reported by name.
    #[test]
    fn invalid_pattern_is_named() {
        let config = Rules {
            guarded: vec!["ok".to_owned(), "(unclosed".to_owned()],
            exempt: vec![],
            reason: String::new(),
        };
        let error = Policy::compile(config).unwrap_err();
        assert!(matches!(&error, Error::PatternInvalid { pattern, .. } if pattern == "(unclosed"));
        assert!(error.to_string().contains("`(unclosed`"));
    }

    /// Every `{command}` in the reason becomes the denied command.
    #[test]
    fn reason_names_the_command() {
        let denial =
            policy(&["make"], &[], "`{command}` needs the guard; {command}!").decide("make check");
        assert_eq!(
            denial,
            Some(Denial {
                reason: "`make check` needs the guard; make check!".to_owned()
            })
        );
    }

    /// `memguard run` is exempt even when the rules list no exemptions.
    #[test]
    fn memguard_run_is_always_exempt() {
        let policy = policy(&["pytest"], &[], "no");
        assert!(policy.decide("pytest -x").is_some());
        assert!(policy.decide("memguard run 4G pytest -x").is_none());
        assert!(
            policy
                .decide("cd app && /usr/local/bin/memguard run 4G pytest")
                .is_none()
        );
    }

    /// `^` and `$` match at line breaks, as Python's `re.M` does.
    #[test]
    fn patterns_are_multi_line() {
        let policy = policy(&["^pytest$"], &[], "no");
        assert!(policy.decide("cd app\npytest\necho done").is_some());
    }

    /// The rules survive a trip through JSON unchanged.
    #[test]
    fn rules_round_trip_through_json() {
        let config: Rules = toml::from_str(EXAMPLE).expect("the example rules parse");
        let json = serde_json::to_string(&config).expect("serialises");
        assert_eq!(
            serde_json::from_str::<Rules>(&json).expect("deserialises"),
            config
        );
    }

    /// The shipped example parses into the cargo rules.
    #[test]
    fn example_rules_parse_from_toml() {
        let config: Rules = toml::from_str(EXAMPLE).expect("the example rules parse");
        assert_eq!(config.guarded.len(), 2);
        assert_eq!(config.exempt, ["--no-run"]);
        assert!(
            config
                .reason
                .starts_with("`{command}` runs tests or a binary without the memory guard.")
        );
        assert!(!config.reason.contains('\n'));
    }

    /// A misspelt key is an error that names it, not a silently ignored rule.
    #[test]
    fn unknown_keys_are_rejected() {
        let result = toml::from_str::<Rules>("guarded = []\nreason = \"\"\nexmept = []\n");
        assert!(result.unwrap_err().to_string().contains("exmept"));
    }

    /// Rules without `exempt` has no exemptions.
    #[test]
    fn exempt_may_be_omitted() {
        let config: Rules = toml::from_str("guarded = []\nreason = \"\"\n").expect("parses");
        assert!(config.exempt.is_empty());
    }

    /// Up to a few patterns, valid ones from a fixed pool or escaped literals.
    fn patterns() -> impl Strategy<Value = Vec<String>> {
        let pattern = prop_oneof![
            Just(r"^\S+".to_owned()),
            Just(".".to_owned()),
            Just(r"(^|[;&|]\s*)cargo\s+test\b".to_owned()),
            ".{0,8}".prop_map(|literal| regex::escape(&literal)),
        ];
        prop::collection::vec(pattern, 0..4)
    }

    proptest! {
        /// Prefixing `memguard run` allows any command under any rules.
        #[test]
        fn memguard_run_prefix_never_denies(guarded in patterns(), exempt in patterns(), command in ".*") {
            let policy = Policy::compile(Rules { guarded, exempt, reason: String::new() }).expect("compiles");
            prop_assert_eq!(policy.decide(&format!("memguard run 10G {command}")), None);
        }

        /// The shipped rules allow any command run through `memguard run`.
        #[test]
        fn example_never_denies_with_memguard_run_prefix(command in ".*") {
            let guarded = format!("memguard run 10G {command}");
            prop_assert!(!denied(&guarded));
        }

        /// The shipped rules allow any command that only builds.
        #[test]
        fn example_never_denies_no_run(command in ".*") {
            let build_only = format!("{command} --no-run");
            prop_assert!(!denied(&build_only));
        }

        /// The shipped rules allow any command that mentions neither cargo nor `target/`.
        #[test]
        fn example_never_denies_without_cargo_or_target(command in ".*") {
            prop_assume!(!command.contains("cargo") && !command.contains("target/"));
            prop_assert!(!denied(&command));
        }

        /// With no guarded patterns nothing is denied, whatever the exemptions.
        #[test]
        fn empty_guarded_never_denies(exempt in patterns(), command in ".*") {
            let policy = Policy::compile(Rules { guarded: vec![], exempt, reason: String::new() }).expect("compiles");
            prop_assert_eq!(policy.decide(&command), None);
        }

        /// A command matching an exempt pattern is allowed, whatever the guarded patterns.
        #[test]
        fn exempt_match_never_denies(guarded in patterns(), token in ".{1,8}", before in ".*", after in ".*") {
            let exempt = vec![regex::escape(&token)];
            let policy = Policy::compile(Rules { guarded, exempt, reason: String::new() }).expect("compiles");
            prop_assert_eq!(policy.decide(&format!("{before}{token}{after}")), None);
        }
    }
}
