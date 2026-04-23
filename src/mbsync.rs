//! `humos-mail fetch` wraps `mbsync` (isync). This module:
//!
//! 1. Builds an [`MbsyncAccount`] from each account directory under
//!    `~/.humOS/mail/<name>/` (`imap.host`, `imap.port`, `address`, auth).
//! 2. Renders a `.mbsyncrc` from those accounts via [`mbsync_config::render`].
//! 3. Writes it to `<humos_root>/mbsyncrc` — a derived artifact, always
//!    regenerated; the file-based DB stays canonical.
//! 4. Shells out to `mbsync -c <path> (-a | <channel>)` via [`Runner`].

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::account::is_fetchable;
use crate::auth::AuthSource;
use crate::mail::discover_accounts;
use crate::mbsync_config::{self, MbsyncAccount};
use crate::shell::{Runner, ShellCmd};

/// What to fetch: every fetchable account, or a single one by name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    All,
    One(String),
}

/// Read one account's IMAP settings into an [`MbsyncAccount`].
pub fn load_account(mail_root: &Path, name: &str) -> Result<MbsyncAccount> {
    let dir = mail_root.join(name);
    let imap_host = read_fact(&dir, "imap.host")?;
    let imap_port: u16 = read_fact(&dir, "imap.port")?
        .parse()
        .with_context(|| format!("parsing imap.port for account {name:?}"))?;
    let imap_user = read_fact(&dir, "address")?;
    Ok(MbsyncAccount {
        name: name.to_string(),
        imap_host,
        imap_port,
        imap_user,
        maildir_path: dir.clone(),
        auth: AuthSource::from_account_dir(&dir, name),
    })
}

/// Discover every account under `mail_root` that has enough config to sync
/// (imap.host + address). Return them in a stable order.
pub fn load_fetchable_accounts(mail_root: &Path) -> Result<Vec<MbsyncAccount>> {
    let mut out = Vec::new();
    for acct in discover_accounts(mail_root) {
        let dir = mail_root.join(&acct.name);
        if !is_fetchable(&dir) {
            continue;
        }
        out.push(load_account(mail_root, &acct.name)?);
    }
    Ok(out)
}

/// Build the argv for an `mbsync` invocation.
pub fn build_argv(config_path: &Path, target: &Target) -> Vec<String> {
    let mut args = vec!["-c".to_string(), config_path.display().to_string()];
    match target {
        Target::All => args.push("-a".to_string()),
        Target::One(name) => args.push(name.clone()),
    }
    args
}

/// Render the mbsync config into `<humos_root>/mbsyncrc` and exec `mbsync`.
/// Returns the exit code from mbsync.
pub fn fetch(humos_root: &Path, target: &Target, runner: &dyn Runner) -> Result<i32> {
    let mail_root = humos_root.join("mail");
    let accounts = load_fetchable_accounts(&mail_root)?;
    if accounts.is_empty() {
        anyhow::bail!(
            "no fetchable accounts under {} (each account needs imap.host and address)",
            mail_root.display()
        );
    }

    if let Target::One(name) = target {
        if !accounts.iter().any(|a| &a.name == name) {
            anyhow::bail!("account {name:?} is not fetchable or does not exist");
        }
    }

    let config_path = write_config(humos_root, &accounts)?;
    let args = build_argv(&config_path, target);
    runner.run(&ShellCmd::new("mbsync", args))
}

fn write_config(humos_root: &Path, accounts: &[MbsyncAccount]) -> Result<PathBuf> {
    fs::create_dir_all(humos_root)
        .with_context(|| format!("creating {}", humos_root.display()))?;
    let path = humos_root.join("mbsyncrc");
    let content = mbsync_config::render(accounts);
    fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

fn read_fact(dir: &Path, fact: &str) -> Result<String> {
    let path = dir.join(fact);
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(raw.trim().to_string())
}

// ================= tests =================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::test_support::FakeRunner;
    use std::fs;

    fn write_account_files(dir: &Path, email: &str, host: &str, port: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("address"), format!("{email}\n")).unwrap();
        fs::write(dir.join("imap.host"), format!("{host}\n")).unwrap();
        fs::write(dir.join("imap.port"), format!("{port}\n")).unwrap();
    }

    // ---- load_account ----
    #[test]
    fn load_account_reads_all_fields_from_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let mail_root = tmp.path().join("mail");
        let dir = mail_root.join("gmail");
        write_account_files(&dir, "felix@gmail.com", "imap.gmail.com", "993");

        let a = load_account(&mail_root, "gmail").unwrap();
        assert_eq!(a.name, "gmail");
        assert_eq!(a.imap_host, "imap.gmail.com");
        assert_eq!(a.imap_port, 993);
        assert_eq!(a.imap_user, "felix@gmail.com");
        assert_eq!(a.maildir_path, dir);
    }

    #[test]
    fn load_account_defaults_to_keyring_auth() {
        let tmp = tempfile::tempdir().unwrap();
        let mail_root = tmp.path().join("mail");
        let dir = mail_root.join("gmail");
        write_account_files(&dir, "f@g.com", "imap.g.com", "993");

        let a = load_account(&mail_root, "gmail").unwrap();
        assert_eq!(
            a.auth,
            AuthSource::Keyring {
                service: "humos".into(),
                account: "gmail".into()
            }
        );
    }

    #[test]
    fn load_account_respects_password_cmd_override() {
        let tmp = tempfile::tempdir().unwrap();
        let mail_root = tmp.path().join("mail");
        let dir = mail_root.join("gmail");
        write_account_files(&dir, "f@g.com", "imap.g.com", "993");
        fs::write(dir.join("password.cmd"), "pass show humos/gmail\n").unwrap();

        let a = load_account(&mail_root, "gmail").unwrap();
        assert_eq!(a.auth, AuthSource::Cmd("pass show humos/gmail".into()));
    }

    #[test]
    fn load_account_missing_fact_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let mail_root = tmp.path().join("mail");
        let dir = mail_root.join("gmail");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("address"), "x@y").unwrap();
        // no imap.host
        let err = load_account(&mail_root, "gmail").unwrap_err();
        assert!(err.to_string().contains("imap.host"));
    }

    #[test]
    fn load_account_non_numeric_port_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let mail_root = tmp.path().join("mail");
        let dir = mail_root.join("gmail");
        write_account_files(&dir, "x@y", "imap.x.com", "not-a-port");
        assert!(load_account(&mail_root, "gmail").is_err());
    }

    // ---- load_fetchable_accounts ----
    #[test]
    fn load_fetchable_accounts_skips_accounts_missing_imap_config() {
        let tmp = tempfile::tempdir().unwrap();
        let mail_root = tmp.path().join("mail");
        // gmail: fully configured
        write_account_files(&mail_root.join("gmail"), "f@g", "imap.g", "993");
        // icloud: only exists as an empty dir (stub)
        fs::create_dir_all(mail_root.join("icloud")).unwrap();

        let accounts = load_fetchable_accounts(&mail_root).unwrap();
        let names: Vec<_> = accounts.iter().map(|a| a.name.clone()).collect();
        assert_eq!(names, vec!["gmail"]);
    }

    #[test]
    fn load_fetchable_accounts_empty_root_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let accounts = load_fetchable_accounts(&tmp.path().join("nope")).unwrap();
        assert!(accounts.is_empty());
    }

    // ---- build_argv ----
    #[test]
    fn build_argv_all_uses_dash_a() {
        let args = build_argv(Path::new("/tmp/mbsyncrc"), &Target::All);
        assert_eq!(args, vec!["-c", "/tmp/mbsyncrc", "-a"]);
    }

    #[test]
    fn build_argv_one_uses_channel_name() {
        let args = build_argv(Path::new("/tmp/mbsyncrc"), &Target::One("gmail".into()));
        assert_eq!(args, vec!["-c", "/tmp/mbsyncrc", "gmail"]);
    }

    // ---- fetch (orchestrator) ----
    #[test]
    fn fetch_writes_config_and_invokes_mbsync_with_correct_argv_for_all() {
        let tmp = tempfile::tempdir().unwrap();
        let humos_root = tmp.path();
        let mail_root = humos_root.join("mail");
        write_account_files(&mail_root.join("gmail"), "f@g.com", "imap.g.com", "993");

        let runner = FakeRunner::new();
        let code = fetch(humos_root, &Target::All, &runner).unwrap();
        assert_eq!(code, 0);

        // config was written
        let config_path = humos_root.join("mbsyncrc");
        assert!(config_path.is_file());
        let content = fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("IMAPAccount gmail"));
        assert!(content.contains("Host imap.g.com"));

        // runner got correct call
        let last = runner.last();
        assert_eq!(last.program, "mbsync");
        assert_eq!(last.args[0], "-c");
        assert_eq!(last.args[1], config_path.display().to_string());
        assert_eq!(last.args[2], "-a");
        assert!(last.stdin.is_none());
    }

    #[test]
    fn fetch_one_targets_specific_channel() {
        let tmp = tempfile::tempdir().unwrap();
        let humos_root = tmp.path();
        let mail_root = humos_root.join("mail");
        write_account_files(&mail_root.join("gmail"), "f@g", "imap.g", "993");
        write_account_files(&mail_root.join("work"), "f@w", "imap.w", "993");

        let runner = FakeRunner::new();
        fetch(humos_root, &Target::One("gmail".into()), &runner).unwrap();

        let last = runner.last();
        assert_eq!(last.args.last().unwrap(), "gmail");
    }

    #[test]
    fn fetch_no_accounts_errors_without_invoking_runner() {
        let tmp = tempfile::tempdir().unwrap();
        let humos_root = tmp.path();
        fs::create_dir_all(humos_root.join("mail")).unwrap();
        let runner = FakeRunner::new();
        let err = fetch(humos_root, &Target::All, &runner).unwrap_err();
        assert!(err.to_string().contains("no fetchable accounts"));
        assert_eq!(runner.call_count(), 0);
    }

    #[test]
    fn fetch_one_unknown_account_errors_without_invoking_runner() {
        let tmp = tempfile::tempdir().unwrap();
        let humos_root = tmp.path();
        let mail_root = humos_root.join("mail");
        write_account_files(&mail_root.join("gmail"), "f@g", "imap.g", "993");

        let runner = FakeRunner::new();
        let err = fetch(humos_root, &Target::One("nope".into()), &runner).unwrap_err();
        assert!(err.to_string().contains("not fetchable"));
        assert_eq!(runner.call_count(), 0);
    }

    #[test]
    fn fetch_propagates_runner_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        let humos_root = tmp.path();
        let mail_root = humos_root.join("mail");
        write_account_files(&mail_root.join("gmail"), "f@g", "imap.g", "993");

        let runner = FakeRunner::with_exit(3);
        let code = fetch(humos_root, &Target::All, &runner).unwrap();
        assert_eq!(code, 3);
    }
}
