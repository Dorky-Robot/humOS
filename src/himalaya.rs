//! `humos-mail send` wraps `himalaya`. This module:
//!
//! 1. Builds a [`HimalayaAccount`] from each account directory under
//!    `~/.humOS/mail/<name>/` (`address`, `smtp.host`, `smtp.port`, auth,
//!    optional `smtp.login` override).
//! 2. Renders a `config.toml` from those accounts via [`himalaya_config::render`].
//! 3. Writes it to `<humos_root>/himalaya.toml` — a derived artifact, always
//!    regenerated; the file-based DB stays canonical.
//! 4. Shells out to `himalaya -c <path> message send [-a <account>]`,
//!    piping the caller's RFC-822 message on stdin.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::auth::AuthSource;
use crate::himalaya_config::{self, HimalayaAccount};
use crate::mail::discover_accounts;
use crate::shell::{Runner, ShellCmd};

/// Which account to send from: the first configured (default) or a named one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FromAccount {
    Default,
    Named(String),
}

/// Read one account's SMTP settings into a [`HimalayaAccount`].
///
/// `smtp.login` is optional — if absent, the `address` file is used
/// (Gmail/iCloud/Outlook/Yahoo/Fastmail all treat the email as the SMTP user).
pub fn load_account(mail_root: &Path, name: &str) -> Result<HimalayaAccount> {
    let dir = mail_root.join(name);
    let email = read_fact(&dir, "address")?;
    let smtp_host = read_fact(&dir, "smtp.host")?;
    let smtp_port: u16 = read_fact(&dir, "smtp.port")?
        .parse()
        .with_context(|| format!("parsing smtp.port for account {name:?}"))?;
    let smtp_login = match read_optional_fact(&dir, "smtp.login") {
        Some(login) => login,
        None => email.clone(),
    };
    Ok(HimalayaAccount {
        name: name.to_string(),
        email,
        maildir_path: dir.clone(),
        smtp_host,
        smtp_port,
        smtp_login,
        auth: AuthSource::from_account_dir(&dir, name),
    })
}

/// Whether an account has enough config to send mail.
pub fn is_sendable(account_dir: &Path) -> bool {
    account_dir.join("address").is_file()
        && account_dir.join("smtp.host").is_file()
        && account_dir.join("smtp.port").is_file()
}

/// Discover every account under `mail_root` that has enough config to send.
pub fn load_sendable_accounts(mail_root: &Path) -> Result<Vec<HimalayaAccount>> {
    let mut out = Vec::new();
    for acct in discover_accounts(mail_root) {
        let dir = mail_root.join(&acct.name);
        if !is_sendable(&dir) {
            continue;
        }
        out.push(load_account(mail_root, &acct.name)?);
    }
    Ok(out)
}

/// Build the argv for a `himalaya message send` invocation.
pub fn build_send_argv(config_path: &Path, from: &FromAccount) -> Vec<String> {
    let mut args = vec!["-c".to_string(), config_path.display().to_string()];
    if let FromAccount::Named(name) = from {
        args.push("-a".to_string());
        args.push(name.clone());
    }
    args.push("message".to_string());
    args.push("send".to_string());
    args
}

/// Render himalaya config, write it, exec `himalaya message send`, pipe
/// `message` on stdin. Returns the exit code from himalaya.
pub fn send(
    humos_root: &Path,
    from: &FromAccount,
    message: Vec<u8>,
    runner: &dyn Runner,
) -> Result<i32> {
    let mail_root = humos_root.join("mail");
    let accounts = load_sendable_accounts(&mail_root)?;
    if accounts.is_empty() {
        anyhow::bail!(
            "no sendable accounts under {} (each account needs address, smtp.host, smtp.port)",
            mail_root.display()
        );
    }
    if let FromAccount::Named(name) = from {
        if !accounts.iter().any(|a| &a.name == name) {
            anyhow::bail!("account {name:?} is not sendable or does not exist");
        }
    }

    let config_path = write_config(humos_root, &accounts)?;
    let args = build_send_argv(&config_path, from);
    runner.run(&ShellCmd::new("himalaya", args).with_stdin(message))
}

fn write_config(humos_root: &Path, accounts: &[HimalayaAccount]) -> Result<PathBuf> {
    fs::create_dir_all(humos_root)
        .with_context(|| format!("creating {}", humos_root.display()))?;
    let path = humos_root.join("himalaya.toml");
    let content = himalaya_config::render(accounts);
    fs::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

fn read_fact(dir: &Path, fact: &str) -> Result<String> {
    let path = dir.join(fact);
    let raw = fs::read_to_string(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    Ok(raw.trim().to_string())
}

fn read_optional_fact(dir: &Path, fact: &str) -> Option<String> {
    let raw = fs::read_to_string(dir.join(fact)).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

// ================= tests =================
#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::test_support::FakeRunner;

    fn write_account_files(dir: &Path, email: &str, smtp_host: &str, smtp_port: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("address"), format!("{email}\n")).unwrap();
        fs::write(dir.join("smtp.host"), format!("{smtp_host}\n")).unwrap();
        fs::write(dir.join("smtp.port"), format!("{smtp_port}\n")).unwrap();
    }

    // ---- load_account ----
    #[test]
    fn load_account_reads_all_fields_and_defaults_login_to_address() {
        let tmp = tempfile::tempdir().unwrap();
        let mail_root = tmp.path().join("mail");
        write_account_files(&mail_root.join("gmail"), "felix@gmail.com", "smtp.gmail.com", "587");

        let a = load_account(&mail_root, "gmail").unwrap();
        assert_eq!(a.name, "gmail");
        assert_eq!(a.email, "felix@gmail.com");
        assert_eq!(a.smtp_host, "smtp.gmail.com");
        assert_eq!(a.smtp_port, 587);
        assert_eq!(
            a.smtp_login, "felix@gmail.com",
            "smtp.login should default to address"
        );
    }

    #[test]
    fn load_account_respects_explicit_smtp_login_override() {
        let tmp = tempfile::tempdir().unwrap();
        let mail_root = tmp.path().join("mail");
        let dir = mail_root.join("work");
        write_account_files(&dir, "felix@work.com", "smtp.work.com", "587");
        fs::write(dir.join("smtp.login"), "felix.flores\n").unwrap();

        let a = load_account(&mail_root, "work").unwrap();
        assert_eq!(a.smtp_login, "felix.flores");
    }

    #[test]
    fn load_account_empty_smtp_login_falls_back_to_address() {
        let tmp = tempfile::tempdir().unwrap();
        let mail_root = tmp.path().join("mail");
        let dir = mail_root.join("gmail");
        write_account_files(&dir, "felix@gmail.com", "smtp.gmail.com", "587");
        fs::write(dir.join("smtp.login"), "   \n").unwrap();

        let a = load_account(&mail_root, "gmail").unwrap();
        assert_eq!(a.smtp_login, "felix@gmail.com");
    }

    #[test]
    fn load_account_missing_smtp_host_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let mail_root = tmp.path().join("mail");
        let dir = mail_root.join("gmail");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("address"), "x@y").unwrap();
        fs::write(dir.join("smtp.port"), "587").unwrap();
        assert!(load_account(&mail_root, "gmail").is_err());
    }

    // ---- is_sendable ----
    #[test]
    fn is_sendable_true_when_all_three_files_present() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("address"), "x@y").unwrap();
        fs::write(tmp.path().join("smtp.host"), "smtp.y").unwrap();
        fs::write(tmp.path().join("smtp.port"), "587").unwrap();
        assert!(is_sendable(tmp.path()));
    }

    #[test]
    fn is_sendable_false_when_missing_smtp_host() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("address"), "x@y").unwrap();
        fs::write(tmp.path().join("smtp.port"), "587").unwrap();
        assert!(!is_sendable(tmp.path()));
    }

    // ---- load_sendable_accounts ----
    #[test]
    fn load_sendable_accounts_skips_accounts_missing_smtp_config() {
        let tmp = tempfile::tempdir().unwrap();
        let mail_root = tmp.path().join("mail");
        write_account_files(&mail_root.join("gmail"), "f@g", "smtp.g", "587");
        fs::create_dir_all(mail_root.join("icloud")).unwrap(); // stub

        let accounts = load_sendable_accounts(&mail_root).unwrap();
        let names: Vec<_> = accounts.iter().map(|a| a.name.clone()).collect();
        assert_eq!(names, vec!["gmail"]);
    }

    // ---- build_send_argv ----
    #[test]
    fn build_send_argv_default_omits_account_flag() {
        let args = build_send_argv(Path::new("/tmp/h.toml"), &FromAccount::Default);
        assert_eq!(args, vec!["-c", "/tmp/h.toml", "message", "send"]);
    }

    #[test]
    fn build_send_argv_named_includes_dash_a_account() {
        let args = build_send_argv(Path::new("/tmp/h.toml"), &FromAccount::Named("work".into()));
        assert_eq!(args, vec!["-c", "/tmp/h.toml", "-a", "work", "message", "send"]);
    }

    // ---- send (orchestrator) ----
    #[test]
    fn send_writes_config_and_invokes_himalaya_with_stdin_body() {
        let tmp = tempfile::tempdir().unwrap();
        let humos_root = tmp.path();
        let mail_root = humos_root.join("mail");
        write_account_files(&mail_root.join("gmail"), "f@g.com", "smtp.g.com", "587");

        let runner = FakeRunner::new();
        let body = b"From: f@g.com\r\nTo: x@y.com\r\nSubject: hi\r\n\r\nbody\r\n".to_vec();
        let code = send(humos_root, &FromAccount::Default, body.clone(), &runner).unwrap();
        assert_eq!(code, 0);

        let config_path = humos_root.join("himalaya.toml");
        assert!(config_path.is_file());
        let content = fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("[accounts.gmail]"));
        assert!(content.contains("smtp.g.com"));

        let last = runner.last();
        assert_eq!(last.program, "himalaya");
        assert_eq!(last.args[0], "-c");
        assert_eq!(last.args[1], config_path.display().to_string());
        assert_eq!(&last.args[last.args.len() - 2..], &["message", "send"]);
        assert_eq!(last.stdin.as_deref(), Some(body.as_slice()));
    }

    #[test]
    fn send_named_account_passes_dash_a() {
        let tmp = tempfile::tempdir().unwrap();
        let humos_root = tmp.path();
        let mail_root = humos_root.join("mail");
        write_account_files(&mail_root.join("gmail"), "f@g", "smtp.g", "587");
        write_account_files(&mail_root.join("work"), "f@w", "smtp.w", "587");

        let runner = FakeRunner::new();
        send(
            humos_root,
            &FromAccount::Named("work".into()),
            b"msg".to_vec(),
            &runner,
        )
        .unwrap();

        let last = runner.last();
        let joined = last.args.join(" ");
        assert!(joined.contains("-a work"), "argv was {:?}", last.args);
    }

    #[test]
    fn send_no_accounts_errors_without_invoking_runner() {
        let tmp = tempfile::tempdir().unwrap();
        let humos_root = tmp.path();
        fs::create_dir_all(humos_root.join("mail")).unwrap();
        let runner = FakeRunner::new();
        let err = send(humos_root, &FromAccount::Default, b"x".to_vec(), &runner).unwrap_err();
        assert!(err.to_string().contains("no sendable accounts"));
        assert_eq!(runner.call_count(), 0);
    }

    #[test]
    fn send_unknown_account_errors_without_invoking_runner() {
        let tmp = tempfile::tempdir().unwrap();
        let humos_root = tmp.path();
        let mail_root = humos_root.join("mail");
        write_account_files(&mail_root.join("gmail"), "f@g", "smtp.g", "587");
        let runner = FakeRunner::new();
        let err = send(
            humos_root,
            &FromAccount::Named("nope".into()),
            b"x".to_vec(),
            &runner,
        )
        .unwrap_err();
        assert!(err.to_string().contains("not sendable"));
        assert_eq!(runner.call_count(), 0);
    }

    #[test]
    fn send_propagates_runner_exit_code() {
        let tmp = tempfile::tempdir().unwrap();
        let humos_root = tmp.path();
        let mail_root = humos_root.join("mail");
        write_account_files(&mail_root.join("gmail"), "f@g", "smtp.g", "587");
        let runner = FakeRunner::with_exit(5);
        let code = send(humos_root, &FromAccount::Default, b"x".to_vec(), &runner).unwrap();
        assert_eq!(code, 5);
    }
}
