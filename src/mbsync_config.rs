//! Generate an `.mbsyncrc` from humOS's per-account file-based DB.
//!
//! mbsync (isync) is the IMAP ↔ Maildir syncer. humOS uses it to keep
//! `~/.humOS/mail/{account}/INBOX/` populated from the server so that
//! himalaya (maildir backend) can operate offline.
//!
//! Like `himalaya_config::render`, this is a pure function. `humos-mail fetch`
//! (the caller) writes the result to `~/.mbsyncrc` (or `$XDG_CONFIG_HOME/
//! isyncrc`) before invoking `mbsync`.
//!
//! This targets the terminology introduced in isync 1.4 (`Far` / `Near`,
//! `TLSType`); we install `isync >= 1.5.1` via brew.

use crate::auth::{to_shell_cmd, AuthSource};
use std::path::PathBuf;

/// One mail account's IMAP + Maildir wiring.
#[derive(Debug, Clone)]
pub struct MbsyncAccount {
    pub name: String,
    pub imap_host: String,
    pub imap_port: u16,
    pub imap_user: String,
    pub maildir_path: PathBuf,
    pub auth: AuthSource,
}

/// Render accounts into the content of a `.mbsyncrc`.
pub fn render(accounts: &[MbsyncAccount]) -> String {
    let mut out = String::new();
    for (idx, a) in accounts.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        // IMAPAccount: connection details.
        out.push_str(&format!("IMAPAccount {}\n", a.name));
        out.push_str(&format!("Host {}\n", a.imap_host));
        out.push_str(&format!("Port {}\n", a.imap_port));
        out.push_str(&format!("User {}\n", a.imap_user));
        out.push_str(&format!("PassCmd \"{}\"\n", to_shell_cmd(&a.auth)));
        out.push_str("TLSType IMAPS\n");
        out.push_str("AuthMechs LOGIN\n");

        // IMAPStore: the remote (far) side.
        out.push_str(&format!("\nIMAPStore {}-remote\n", a.name));
        out.push_str(&format!("Account {}\n", a.name));

        // MaildirStore: the local (near) side.
        let path = a.maildir_path.display().to_string();
        let path_with_slash = if path.ends_with('/') {
            path.clone()
        } else {
            format!("{path}/")
        };
        let inbox = format!("{path_with_slash}INBOX");
        out.push_str(&format!("\nMaildirStore {}-local\n", a.name));
        out.push_str(&format!("Path {path_with_slash}\n"));
        out.push_str(&format!("Inbox {inbox}\n"));
        out.push_str("SubFolders Verbatim\n");

        // Channel: the sync rule.
        out.push_str(&format!("\nChannel {}\n", a.name));
        out.push_str(&format!("Far :{}-remote:\n", a.name));
        out.push_str(&format!("Near :{}-local:\n", a.name));
        // Pull-only: never upload or delete on server. See the
        // channel_block_is_pull_only test for the rationale.
        out.push_str("Patterns *\n");
        out.push_str("Create Near\n");
        out.push_str("SyncState *\n");
        out.push_str("Expunge Near\n");
    }
    out
}

// ================= tests =================
#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: &str) -> MbsyncAccount {
        MbsyncAccount {
            name: name.to_string(),
            imap_host: format!("imap.{name}.com"),
            imap_port: 993,
            imap_user: format!("felix@{name}.com"),
            maildir_path: PathBuf::from(format!("/home/felix/.humOS/mail/{name}")),
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
    fn single_account_has_imapaccount_block() {
        let out = render(&[sample("gmail")]);
        for needle in [
            "IMAPAccount gmail",
            "Host imap.gmail.com",
            "Port 993",
            "User felix@gmail.com",
        ] {
            assert!(out.contains(needle), "missing {needle:?} in:\n{out}");
        }
    }

    #[test]
    fn keyring_auth_renders_passcmd_with_platform_keychain() {
        let out = render(&[sample("gmail")]);
        assert!(
            out.contains(
                "PassCmd \"security find-generic-password -s humos -a gmail -w\""
            ),
            "output was:\n{out}"
        );
    }

    #[test]
    fn cmd_auth_renders_passcmd_with_given_cmd() {
        let mut acc = sample("gmail");
        acc.auth = AuthSource::Cmd("pass show humos/gmail".into());
        let out = render(&[acc]);
        assert!(
            out.contains("PassCmd \"pass show humos/gmail\""),
            "output was:\n{out}"
        );
    }

    #[test]
    fn imapaccount_block_declares_tls_and_auth_mechs() {
        let out = render(&[sample("gmail")]);
        assert!(out.contains("TLSType IMAPS"), "output was:\n{out}");
        assert!(out.contains("AuthMechs LOGIN"), "output was:\n{out}");
    }

    #[test]
    fn imapstore_block_is_bound_to_account() {
        let out = render(&[sample("gmail")]);
        assert!(out.contains("IMAPStore gmail-remote"), "output was:\n{out}");
        assert!(out.contains("Account gmail"), "output was:\n{out}");
    }

    #[test]
    fn maildirstore_block_has_path_inbox_and_subfolders() {
        let out = render(&[sample("gmail")]);
        for needle in [
            "MaildirStore gmail-local",
            "Path /home/felix/.humOS/mail/gmail/",
            "Inbox /home/felix/.humOS/mail/gmail/INBOX",
            "SubFolders Verbatim",
        ] {
            assert!(out.contains(needle), "missing {needle:?} in:\n{out}");
        }
    }

    #[test]
    fn maildir_path_gets_trailing_slash_when_caller_omits_one() {
        let mut acc = sample("gmail");
        acc.maildir_path = PathBuf::from("/home/felix/.humOS/mail/gmail");
        let out = render(&[acc]);
        assert!(
            out.contains("Path /home/felix/.humOS/mail/gmail/"),
            "output was:\n{out}"
        );
    }

    #[test]
    fn channel_block_wires_far_and_near_stores() {
        let out = render(&[sample("gmail")]);
        assert!(out.contains("Channel gmail"), "output was:\n{out}");
        assert!(out.contains("Far :gmail-remote:"), "output was:\n{out}");
        assert!(out.contains("Near :gmail-local:"), "output was:\n{out}");
    }

    #[test]
    fn channel_block_is_pull_only() {
        // humOS uses mbsync as a downstream pull (server → local) only. Sending
        // goes through himalaya/SMTP, not mbsync. So creates and expunges must
        // not propagate from local to remote — otherwise mbsync will upload any
        // local message without a remote UID mapping (the v0.1.0 footgun that
        // created 506 duplicates in a user's Gmail on first sync).
        let out = render(&[sample("gmail")]);
        for needle in ["Patterns *", "Create Near", "SyncState *", "Expunge Near"] {
            assert!(out.contains(needle), "missing {needle:?} in:\n{out}");
        }
        assert!(
            !out.contains("Create Both"),
            "must not Create Both (would upload local-only messages to server):\n{out}"
        );
        assert!(
            !out.contains("Expunge Both"),
            "must not Expunge Both (would delete server messages when user archives locally):\n{out}"
        );
    }

    #[test]
    fn multiple_accounts_each_get_all_four_blocks() {
        let out = render(&[sample("gmail"), sample("work")]);
        for name in ["gmail", "work"] {
            assert!(out.contains(&format!("IMAPAccount {name}")));
            assert!(out.contains(&format!("IMAPStore {name}-remote")));
            assert!(out.contains(&format!("MaildirStore {name}-local")));
            assert!(out.contains(&format!("Channel {name}")));
        }
    }
}
