//! Shared password-retrieval abstraction used by both the himalaya and
//! mbsync config generators.
//!
//! humOS stores passwords in the OS keyring (via the `keyring` crate) and
//! supports an optional `password.cmd` power-user override per account.
//! When generating third-party tool configs, we always render a shell
//! command: mbsync has no native keyring support, and Homebrew's himalaya
//! bottle is built without the `keyring` feature. Shelling to `security`
//! (macOS), `secret-tool` (Linux), etc., is the portable lowest common
//! denominator.

use std::fs;
use std::path::Path;

/// How a third-party tool should obtain an account password.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthSource {
    /// Password lives in the OS keyring under `(service, account)`.
    Keyring { service: String, account: String },
    /// Explicit shell command whose stdout is the password.
    Cmd(String),
}

impl AuthSource {
    /// Resolve the auth source for one account directory. If a non-empty
    /// `password.cmd` file is present, use it; otherwise default to the
    /// `humos` keyring entry keyed by account name.
    pub fn from_account_dir(account_dir: &Path, account_name: &str) -> Self {
        let cmd_file = account_dir.join("password.cmd");
        if let Ok(raw) = fs::read_to_string(&cmd_file) {
            let cmd = raw.trim();
            if !cmd.is_empty() {
                return AuthSource::Cmd(cmd.to_string());
            }
        }
        AuthSource::Keyring {
            service: "humos".into(),
            account: account_name.into(),
        }
    }
}

/// Shell command that prints the keyring password for `(service, account)`.
/// macOS today; Linux/Windows support to follow when humOS runs there.
pub fn keyring_fetch_cmd(service: &str, account: &str) -> String {
    format!("security find-generic-password -s {service} -a {account} -w")
}

/// Resolve an `AuthSource` to the shell command string to embed in a
/// third-party config.
pub fn to_shell_cmd(auth: &AuthSource) -> String {
    match auth {
        AuthSource::Keyring { service, account } => keyring_fetch_cmd(service, account),
        AuthSource::Cmd(c) => c.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyring_fetch_cmd_targets_macos_security() {
        assert_eq!(
            keyring_fetch_cmd("humos", "gmail"),
            "security find-generic-password -s humos -a gmail -w"
        );
    }

    #[test]
    fn to_shell_cmd_passes_through_cmd_variant() {
        let s = to_shell_cmd(&AuthSource::Cmd("pass show humos/gmail".into()));
        assert_eq!(s, "pass show humos/gmail");
    }

    #[test]
    fn to_shell_cmd_converts_keyring_variant() {
        let s = to_shell_cmd(&AuthSource::Keyring {
            service: "humos".into(),
            account: "work".into(),
        });
        assert_eq!(s, "security find-generic-password -s humos -a work -w");
    }

    // ---- AuthSource::from_account_dir ----
    #[test]
    fn from_account_dir_defaults_to_keyring_when_no_override() {
        let tmp = tempfile::tempdir().unwrap();
        let auth = AuthSource::from_account_dir(tmp.path(), "gmail");
        assert_eq!(
            auth,
            AuthSource::Keyring {
                service: "humos".into(),
                account: "gmail".into(),
            }
        );
    }

    #[test]
    fn from_account_dir_uses_password_cmd_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("password.cmd"), "pass show humos/gmail\n").unwrap();
        let auth = AuthSource::from_account_dir(tmp.path(), "gmail");
        assert_eq!(auth, AuthSource::Cmd("pass show humos/gmail".into()));
    }

    #[test]
    fn from_account_dir_empty_password_cmd_falls_back_to_keyring() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("password.cmd"), "   \n").unwrap();
        let auth = AuthSource::from_account_dir(tmp.path(), "gmail");
        assert!(matches!(auth, AuthSource::Keyring { .. }));
    }
}
