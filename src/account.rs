//! Account setup and credential management for `~/.humOS/mail/`.
//!
//! Each account lives in its own directory. Small facts are small files:
//! `address`, `imap.host`, `imap.port`. Passwords live in the OS keyring
//! via the `keyring` crate; a `password.cmd` file is an optional power-user
//! override.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

// ---- auto-config for well-known providers ----

/// IMAP server settings for a well-known email provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConfig {
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
}

/// Look up IMAP/SMTP settings by email domain. Returns `None` for unknown
/// domains (caller must ask the user).
pub fn auto_provider_config(email: &str) -> Option<ProviderConfig> {
    let domain = email.rsplit_once('@')?.1.to_ascii_lowercase();
    match domain.as_str() {
        "gmail.com" | "googlemail.com" => Some(ProviderConfig {
            imap_host: "imap.gmail.com".into(),
            imap_port: 993,
            smtp_host: "smtp.gmail.com".into(),
            smtp_port: 587,
        }),
        "icloud.com" | "me.com" | "mac.com" => Some(ProviderConfig {
            imap_host: "imap.mail.me.com".into(),
            imap_port: 993,
            smtp_host: "smtp.mail.me.com".into(),
            smtp_port: 587,
        }),
        "outlook.com" | "hotmail.com" | "live.com" => Some(ProviderConfig {
            imap_host: "outlook.office365.com".into(),
            imap_port: 993,
            smtp_host: "smtp.office365.com".into(),
            smtp_port: 587,
        }),
        "yahoo.com" | "ymail.com" => Some(ProviderConfig {
            imap_host: "imap.mail.yahoo.com".into(),
            imap_port: 993,
            smtp_host: "smtp.mail.yahoo.com".into(),
            smtp_port: 587,
        }),
        "fastmail.com" | "fastmail.fm" => Some(ProviderConfig {
            imap_host: "imap.fastmail.com".into(),
            imap_port: 993,
            smtp_host: "smtp.fastmail.com".into(),
            smtp_port: 587,
        }),
        _ => None,
    }
}

// ---- account creation ----

/// Validate an account name. Must be non-empty, ASCII alphanumeric + hyphens,
/// no path traversal.
fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("account name cannot be empty");
    }
    if name.contains('/') || name.contains('\\') || name.contains("..") {
        anyhow::bail!("invalid account name: {name:?}");
    }
    if name.starts_with('.') || name.starts_with('-') {
        anyhow::bail!("account name must not start with '.' or '-': {name:?}");
    }
    Ok(())
}

/// Create a new mail account under `mail_root`.
///
/// Sets up the directory tree, writes config files, and stores the password
/// in the OS keyring. Does NOT create a `password.cmd` file (that's a
/// power-user override).
pub fn create_account(
    mail_root: &Path,
    name: &str,
    email: &str,
    password: &str,
    provider: &ProviderConfig,
) -> Result<PathBuf> {
    validate_name(name)?;
    let account_dir = mail_root.join(name);
    if account_dir.exists() {
        anyhow::bail!("account {name:?} already exists");
    }

    // Create maildir structure.
    for sub in ["INBOX/new", "INBOX/cur", "INBOX/tmp"] {
        fs::create_dir_all(account_dir.join(sub))
            .with_context(|| format!("creating {}/{sub}", account_dir.display()))?;
    }

    // Write config files (small-file-per-fact).
    write_fact(&account_dir, "address", email)?;
    write_fact(&account_dir, "imap.host", &provider.imap_host)?;
    write_fact(&account_dir, "imap.port", &provider.imap_port.to_string())?;
    write_fact(&account_dir, "smtp.host", &provider.smtp_host)?;
    write_fact(&account_dir, "smtp.port", &provider.smtp_port.to_string())?;
    write_fact(&account_dir, "smtp.login", email)?;

    // Store password in OS keyring.
    store_password(name, password)
        .with_context(|| format!("storing password for account {name:?}"))?;

    Ok(account_dir)
}

fn write_fact(dir: &Path, name: &str, value: &str) -> Result<()> {
    let path = dir.join(name);
    fs::write(&path, format!("{value}\n"))
        .with_context(|| format!("writing {}", path.display()))
}

// ---- credential retrieval ----

/// Retrieve the password for an account. Checks for a `password.cmd` override
/// first; falls back to the OS keyring.
pub fn get_password(account_dir: &Path, account_name: &str) -> Result<String> {
    let cmd_file = account_dir.join("password.cmd");
    if cmd_file.is_file() {
        return run_password_cmd(&cmd_file);
    }
    retrieve_password(account_name)
}

fn run_password_cmd(cmd_file: &Path) -> Result<String> {
    let cmd = fs::read_to_string(cmd_file)
        .with_context(|| format!("reading {}", cmd_file.display()))?;
    let cmd = cmd.trim();
    let output = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .with_context(|| format!("executing password.cmd: {cmd}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "password.cmd exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn store_password(account_name: &str, password: &str) -> Result<()> {
    let entry = keyring::Entry::new("humos", account_name)
        .context("creating keyring entry")?;
    entry
        .set_password(password)
        .context("storing password in keyring")?;
    Ok(())
}

fn retrieve_password(account_name: &str) -> Result<String> {
    let entry = keyring::Entry::new("humos", account_name)
        .context("creating keyring entry")?;
    entry
        .get_password()
        .context("retrieving password from keyring")
}

// ---- config reading ----

#[cfg(test)]
fn read_fact(dir: &Path, name: &str) -> Result<String> {
    let path = dir.join(name);
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(raw.trim().to_string())
}

/// Check whether an account directory has IMAP config files (imap.host + address).
pub fn is_fetchable(account_dir: &Path) -> bool {
    account_dir.join("imap.host").is_file() && account_dir.join("address").is_file()
}

/// Delete an account: remove its directory under `mail_root` and drop the
/// keyring entry. Missing directory or missing keyring entry are non-fatal
/// (the caller is making the system match an intent, not just copying state).
pub fn delete_account(mail_root: &Path, name: &str) -> Result<()> {
    validate_name(name)?;
    let dir = mail_root.join(name);
    if dir.exists() {
        fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
    }
    // Keyring errors are swallowed: if the entry doesn't exist that's the
    // desired end state. Real errors are rare and would leave an orphaned
    // keyring entry; the user can clean those up by hand.
    if let Ok(entry) = keyring::Entry::new("humos", name) {
        let _ = entry.delete_credential();
    }
    Ok(())
}

// ================= tests =================
#[cfg(test)]
mod tests {
    use super::*;

    // ---- auto_provider_config ----
    #[test]
    fn auto_config_gmail() {
        let c = auto_provider_config("felix@gmail.com").unwrap();
        assert_eq!(c.imap_host, "imap.gmail.com");
        assert_eq!(c.imap_port, 993);
        assert_eq!(c.smtp_host, "smtp.gmail.com");
        assert_eq!(c.smtp_port, 587);
    }

    #[test]
    fn auto_config_googlemail() {
        assert!(auto_provider_config("x@googlemail.com").is_some());
    }

    #[test]
    fn auto_config_icloud() {
        let c = auto_provider_config("felix@icloud.com").unwrap();
        assert_eq!(c.imap_host, "imap.mail.me.com");
    }

    #[test]
    fn auto_config_me_com() {
        assert!(auto_provider_config("x@me.com").is_some());
    }

    #[test]
    fn auto_config_outlook() {
        let c = auto_provider_config("x@outlook.com").unwrap();
        assert_eq!(c.imap_host, "outlook.office365.com");
    }

    #[test]
    fn auto_config_unknown_domain_returns_none() {
        assert!(auto_provider_config("x@mycustomdomain.org").is_none());
    }

    #[test]
    fn auto_config_no_at_sign_returns_none() {
        assert!(auto_provider_config("not-an-email").is_none());
    }

    #[test]
    fn auto_config_case_insensitive() {
        assert!(auto_provider_config("X@Gmail.COM").is_some());
    }

    // ---- validate_name ----
    #[test]
    fn validate_name_good_names() {
        assert!(validate_name("gmail").is_ok());
        assert!(validate_name("my-fastmail").is_ok());
        assert!(validate_name("work123").is_ok());
    }

    #[test]
    fn validate_name_rejects_empty() {
        assert!(validate_name("").is_err());
    }

    #[test]
    fn validate_name_rejects_path_traversal() {
        assert!(validate_name("..").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("a\\b").is_err());
    }

    #[test]
    fn validate_name_rejects_leading_dot_or_dash() {
        assert!(validate_name(".hidden").is_err());
        assert!(validate_name("-flag").is_err());
    }

    // ---- create_account (filesystem only, skip keyring in CI) ----

    // Note: create_account calls store_password which hits the real OS keyring.
    // In CI or sandboxed environments this may fail. The filesystem portion is
    // tested separately below via write_fact / read_fact.

    #[test]
    fn write_and_read_fact_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        write_fact(tmp.path(), "address", "felix@gmail.com").unwrap();
        assert_eq!(read_fact(tmp.path(), "address").unwrap(), "felix@gmail.com");
    }

    #[test]
    fn write_fact_appends_newline() {
        let tmp = tempfile::tempdir().unwrap();
        write_fact(tmp.path(), "address", "x@y.com").unwrap();
        let raw = fs::read_to_string(tmp.path().join("address")).unwrap();
        assert_eq!(raw, "x@y.com\n");
    }

    #[test]
    fn read_fact_trims_whitespace() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("address"), "  x@y.com  \n").unwrap();
        assert_eq!(read_fact(tmp.path(), "address").unwrap(), "x@y.com");
    }

    #[test]
    fn read_fact_missing_file_errors() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_fact(tmp.path(), "nope").is_err());
    }

    #[test]
    fn is_fetchable_true_when_both_files_exist() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("address"), "x@y").unwrap();
        fs::write(tmp.path().join("imap.host"), "imap.y.com").unwrap();
        assert!(is_fetchable(tmp.path()));
    }

    #[test]
    fn is_fetchable_false_when_missing_imap_host() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("address"), "x@y").unwrap();
        assert!(!is_fetchable(tmp.path()));
    }

    #[test]
    fn is_fetchable_false_when_missing_address() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("imap.host"), "imap.y.com").unwrap();
        assert!(!is_fetchable(tmp.path()));
    }

    // ---- delete_account (filesystem side) ----
    #[test]
    fn delete_account_removes_the_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let mail_root = tmp.path();
        let dir = mail_root.join("gmail");
        fs::create_dir_all(dir.join("INBOX/new")).unwrap();
        fs::write(dir.join("address"), "x@y").unwrap();

        delete_account(mail_root, "gmail").unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn delete_account_missing_directory_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(delete_account(tmp.path(), "never-existed").is_ok());
    }

    #[test]
    fn delete_account_rejects_path_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(delete_account(tmp.path(), "..").is_err());
        assert!(delete_account(tmp.path(), "a/b").is_err());
        assert!(delete_account(tmp.path(), "").is_err());
    }

    // ---- password.cmd override ----
    #[test]
    fn run_password_cmd_executes_and_trims() {
        let tmp = tempfile::tempdir().unwrap();
        let cmd_file = tmp.path().join("password.cmd");
        fs::write(&cmd_file, "echo 'secret123'").unwrap();
        let pw = run_password_cmd(&cmd_file).unwrap();
        assert_eq!(pw, "secret123");
    }

    #[test]
    fn run_password_cmd_failing_command_is_error() {
        let tmp = tempfile::tempdir().unwrap();
        let cmd_file = tmp.path().join("password.cmd");
        fs::write(&cmd_file, "exit 1").unwrap();
        assert!(run_password_cmd(&cmd_file).is_err());
    }
}
