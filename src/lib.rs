//! humOS shared library.
//!
//! humOS treats `~/.humOS/` as a file-based DB. Per-module binaries (humos-mail,
//! humos-web, ...) share the small set of pure functions that read and mutate
//! that on-disk state via this crate.

pub mod account;
pub mod auth;
pub mod cli;
pub mod himalaya;
pub mod himalaya_config;
pub mod mail;
pub mod mbsync;
pub mod mbsync_config;
pub mod prompts;
pub mod shell;
pub mod triage;

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub fn humos_dir_for(home: &Path) -> PathBuf {
    home.join(".humOS")
}

pub fn humos_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("$HOME is not set")?;
    Ok(humos_dir_for(Path::new(&home)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn humos_dir_for_joins_dot_hum_os_onto_home() {
        let home = Path::new("/tmp/fake-home");
        assert_eq!(humos_dir_for(home), PathBuf::from("/tmp/fake-home/.humOS"));
    }
}
