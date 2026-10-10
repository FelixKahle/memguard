//! Where the rules file lives and how it is read.

use std::fs;
use std::path::{Path, PathBuf};

use crate::Error;
use crate::policy::{Policy, Rules};

/// The rules file used when none is named: `~/.config/memguard/rules.toml`; `None` if there is no home directory.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    std::env::home_dir().map(|home| home.join(".config").join("memguard").join("rules.toml"))
}

/// Reads, parses and compiles the rules file at `path`.
///
/// # Errors
///
/// Returns [`Error::RulesUnreadable`] if the file cannot be read (including when it does not exist),
/// [`Error::RulesInvalid`] if it is not valid [`Rules`], and [`Error::PatternInvalid`] if a pattern in it is not a
/// valid regex.
pub fn load(path: impl AsRef<Path>) -> Result<Policy, Error> {
    load_path(path.as_ref())
}

/// The non-generic body of [`load`].
///
/// # Errors
///
/// As [`load`].
fn load_path(path: &Path) -> Result<Policy, Error> {
    let text = fs::read_to_string(path).map_err(|source| Error::RulesUnreadable {
        path: path.to_owned(),
        source,
    })?;
    let rules: Rules = toml::from_str(&text).map_err(|source| Error::RulesInvalid {
        path: path.to_owned(),
        source,
    })?;
    Policy::compile(rules)
}
