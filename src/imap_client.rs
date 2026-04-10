//! Native IMAP client for fetching mail into `~/.humOS/mail/<acct>/INBOX/`.
//!
//! Uses the `imap` crate (sync) + `native-tls`. Fetches unseen messages from
//! INBOX, writes each as a file in `INBOX/new/` using maildir naming. Tracks
//! the last-seen UID in `.last_uid` to avoid re-downloading.
//!
//! This module does NOT speak SMTP. Sending comes in a later slice via `lettre`.

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::account::ImapAccountConfig;

// ---- maildir filename ----

/// Build a maildir-compliant filename: `<timestamp>.<pid>.<hostname>`.
/// The UID is embedded after the hostname so we can track what we've fetched.
pub fn maildir_filename(uid: u32) -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let pid = std::process::id();
    let hostname = "localhost";
    format!("{timestamp}.{pid}.{hostname}.U{uid}")
}

// ---- UID tracking ----

/// Read the last-fetched UID from `.last_uid` in the account dir.
/// Returns 0 if the file doesn't exist (fetch everything).
pub fn read_last_uid(account_dir: &Path) -> u32 {
    let path = account_dir.join(".last_uid");
    fs::read_to_string(path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Write the last-fetched UID.
pub fn write_last_uid(account_dir: &Path, uid: u32) -> Result<()> {
    let path = account_dir.join(".last_uid");
    fs::write(&path, format!("{uid}\n"))
        .with_context(|| format!("writing {}", path.display()))
}

// ---- IMAP fetch ----

/// Connect to IMAP, fetch messages with UID > last_uid, write them to
/// `maildir_new/`, update `.last_uid`. Returns count of new messages.
pub fn fetch_inbox(config: &ImapAccountConfig, account_dir: &Path) -> Result<usize> {
    let maildir_new = account_dir.join("INBOX").join("new");
    fs::create_dir_all(&maildir_new)
        .with_context(|| format!("creating {}", maildir_new.display()))?;

    let last_uid = read_last_uid(account_dir);

    let tls = native_tls::TlsConnector::builder()
        .build()
        .context("building TLS connector")?;
    let client = imap::connect(
        (config.imap_host.as_str(), config.imap_port),
        &config.imap_host,
        &tls,
    )
    .with_context(|| format!("connecting to {}:{}", config.imap_host, config.imap_port))?;

    let mut session = client
        .login(&config.email, &config.password)
        .map_err(|e| anyhow::anyhow!("IMAP login failed: {}", e.0))
        .context("logging in")?;

    session.select("INBOX").context("selecting INBOX")?;

    // Fetch UIDs > last_uid. UID range "<last+1>:*" fetches all newer messages.
    // If last_uid is 0 we fetch everything ("1:*").
    let uid_range = if last_uid == 0 {
        "1:*".to_string()
    } else {
        format!("{}:*", last_uid + 1)
    };

    let messages = session
        .uid_fetch(&uid_range, "RFC822")
        .context("fetching messages")?;

    let mut count = 0u32;
    let mut max_uid = last_uid;

    for msg in messages.iter() {
        let uid = match msg.uid {
            Some(u) => u,
            None => continue,
        };
        // IMAP UID FETCH with "last+1:*" can return the last_uid itself if
        // nothing newer exists. Skip it.
        if uid <= last_uid {
            continue;
        }
        let body = match msg.body() {
            Some(b) => b,
            None => continue,
        };

        let filename = maildir_filename(uid);
        let dest = maildir_new.join(&filename);
        fs::write(&dest, body)
            .with_context(|| format!("writing {}", dest.display()))?;

        if uid > max_uid {
            max_uid = uid;
        }
        count += 1;
    }

    if max_uid > last_uid {
        write_last_uid(account_dir, max_uid)?;
    }

    session.logout().ok();
    Ok(count as usize)
}

// ================= tests =================
#[cfg(test)]
mod tests {
    use super::*;

    // ---- maildir_filename ----
    #[test]
    fn maildir_filename_contains_uid() {
        let name = maildir_filename(42);
        assert!(name.contains(".U42"));
    }

    #[test]
    fn maildir_filename_has_three_dot_separated_parts_plus_uid() {
        let name = maildir_filename(1);
        // format: <timestamp>.<pid>.<hostname>.U<uid>
        let parts: Vec<_> = name.splitn(4, '.').collect();
        assert_eq!(parts.len(), 4);
        assert!(parts[3].starts_with("U"));
    }

    // ---- UID tracking ----
    #[test]
    fn read_last_uid_missing_file_returns_zero() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(read_last_uid(tmp.path()), 0);
    }

    #[test]
    fn write_then_read_last_uid_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        write_last_uid(tmp.path(), 42).unwrap();
        assert_eq!(read_last_uid(tmp.path()), 42);
    }

    #[test]
    fn read_last_uid_trims_whitespace() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".last_uid"), "  100  \n").unwrap();
        assert_eq!(read_last_uid(tmp.path()), 100);
    }

    #[test]
    fn read_last_uid_garbage_returns_zero() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join(".last_uid"), "not-a-number").unwrap();
        assert_eq!(read_last_uid(tmp.path()), 0);
    }

    // Note: fetch_inbox requires a real IMAP server and is tested manually.
    // The pure helpers (maildir_filename, UID tracking) are tested above.
}
