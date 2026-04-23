//! Generate himalaya's `config.toml` from humOS's per-account file-based DB.
//!
//! himalaya's config is a single god-file with account arrays — a shape humOS
//! principles forbid as a source of truth. We generate it from the per-account
//! files under `~/.humOS/mail/{name}/` as a derived artifact. The file-based
//! DB remains canonical; this module produces a string the caller writes to
//! `~/.config/himalaya/config.toml`.
//!
//! himalaya is used in the offline stack with a **Maildir backend** (mbsync
//! populates the Maildir out-of-band) and an **SMTP backend** for sending.

use crate::auth::{to_shell_cmd, AuthSource};
use std::path::PathBuf;

/// One account rendered into a himalaya config section.
#[derive(Debug, Clone)]
pub struct HimalayaAccount {
    pub name: String,
    pub email: String,
    pub maildir_path: PathBuf,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub smtp_login: String,
    pub auth: AuthSource,
}

/// Render accounts into the content of a himalaya `config.toml`.
/// The first account (if any) is marked `default = true`.
pub fn render(accounts: &[HimalayaAccount]) -> String {
    let mut out = String::new();
    for (idx, a) in accounts.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        out.push_str(&format!("[accounts.{}]\n", a.name));
        if idx == 0 {
            out.push_str("default = true\n");
        }
        out.push_str(&format!("email = \"{}\"\n", a.email));
        out.push_str(&format!("display-name = \"{}\"\n", a.email));
        out.push_str("backend.type = \"maildir\"\n");
        out.push_str(&format!(
            "backend.root-dir = \"{}\"\n",
            a.maildir_path.display()
        ));
        out.push('\n');
        out.push_str("message.send.backend.type = \"smtp\"\n");
        out.push_str(&format!("message.send.backend.host = \"{}\"\n", a.smtp_host));
        out.push_str(&format!("message.send.backend.port = {}\n", a.smtp_port));
        out.push_str("message.send.backend.encryption.type = \"start-tls\"\n");
        out.push_str(&format!(
            "message.send.backend.login = \"{}\"\n",
            a.smtp_login
        ));
        out.push_str("message.send.backend.auth.type = \"password\"\n");
        let cmd = to_shell_cmd(&a.auth);
        out.push_str(&format!("message.send.backend.auth.cmd = \"{cmd}\"\n"));
    }
    out
}

// ================= tests =================
#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str) -> HimalayaAccount {
        HimalayaAccount {
            name: name.to_string(),
            email: format!("felix@{name}.com"),
            maildir_path: PathBuf::from(format!("/home/felix/.humOS/mail/{name}")),
            smtp_host: format!("smtp.{name}.com"),
            smtp_port: 587,
            smtp_login: format!("felix@{name}.com"),
            auth: AuthSource::Keyring {
                service: "humos".into(),
                account: name.into(),
            },
        }
    }

    #[test]
    fn empty_accounts_produces_empty_string() {
        assert_eq!(render(&[]), "");
    }

    #[test]
    fn single_account_contains_section_header() {
        let out = render(&[sample("gmail")]);
        assert!(out.contains("[accounts.gmail]"), "output was:\n{out}");
    }

    #[test]
    fn first_account_is_marked_default() {
        let out = render(&[sample("gmail")]);
        assert!(out.contains("default = true"), "output was:\n{out}");
    }

    #[test]
    fn only_first_account_is_default() {
        let out = render(&[sample("gmail"), sample("work")]);
        assert_eq!(
            out.matches("default = true").count(),
            1,
            "expected exactly one default=true line, output was:\n{out}"
        );
    }

    #[test]
    fn account_email_is_rendered() {
        let out = render(&[sample("gmail")]);
        assert!(
            out.contains("email = \"felix@gmail.com\""),
            "output was:\n{out}"
        );
    }

    #[test]
    fn maildir_backend_uses_root_dir() {
        let out = render(&[sample("gmail")]);
        assert!(out.contains("backend.type = \"maildir\""), "output was:\n{out}");
        assert!(
            out.contains("backend.root-dir = \"/home/felix/.humOS/mail/gmail\""),
            "output was:\n{out}"
        );
    }

    #[test]
    fn smtp_backend_is_populated() {
        let out = render(&[sample("gmail")]);
        for needle in [
            "message.send.backend.type = \"smtp\"",
            "message.send.backend.host = \"smtp.gmail.com\"",
            "message.send.backend.port = 587",
            "message.send.backend.login = \"felix@gmail.com\"",
        ] {
            assert!(out.contains(needle), "missing {needle:?} in:\n{out}");
        }
    }

    #[test]
    fn keyring_auth_renders_as_platform_keychain_cmd() {
        let out = render(&[sample("gmail")]);
        // On macOS the keyring is Keychain; we shell out to `security` since
        // Homebrew's himalaya build lacks the `keyring` feature.
        assert!(
            out.contains(
                "message.send.backend.auth.cmd = \"security find-generic-password -s humos -a gmail -w\""
            ),
            "output was:\n{out}"
        );
    }

    #[test]
    fn cmd_auth_renders_as_cmd_field() {
        let mut acc = sample("gmail");
        acc.auth = AuthSource::Cmd("pass show humos/gmail".into());
        let out = render(&[acc]);
        assert!(
            out.contains("message.send.backend.auth.cmd = \"pass show humos/gmail\""),
            "output was:\n{out}"
        );
    }

    #[test]
    fn multiple_accounts_each_get_a_section() {
        let out = render(&[sample("gmail"), sample("work")]);
        assert!(out.contains("[accounts.gmail]"));
        assert!(out.contains("[accounts.work]"));
    }
}
