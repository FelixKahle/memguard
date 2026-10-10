// MIT License
//
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
