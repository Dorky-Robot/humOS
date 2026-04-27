//! Mail module — read and mutate `~/.humOS/mail/`.
//!
//! Layout (per the vision doc):
//!
//! ```text
//! ~/.humOS/mail/
//!   gmail/
//!     address       ← optional, single line, display only
//!     INBOX/
//!       new/  cur/  tmp/
//!     Archive/
//!       cur/        ← created on first archive
//! ```
//!
//! humOS does not speak IMAP. `mbsync` (or whatever the user prefers) is
//! responsible for putting mail under `INBOX/`. This module only ever touches
//! the local filesystem.

use anyhow::{Context, Result};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

// ---- entity model ----

/// One mail account discovered on disk under `~/.humOS/mail/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Account {
    /// Directory name (e.g. "gmail").
    pub name: String,
    /// Contents of `<account_dir>/address`, trimmed; `None` if absent or empty.
    pub address: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Summary {
    pub from: String,
    pub subject: String,
    /// Maildir filename inside `INBOX/new/`. Used as the handle for actions.
    pub filename: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AccountReport {
    pub account: String,
    pub label: String,
    pub unread_count: usize,
    pub read_count: usize,
    pub recent: Vec<Summary>,
}

// ---- account discovery (pure on a path) ----

/// Scan `mail_root` for account directories. Each subdirectory is one account.
/// Hidden directories (starting with `.`) and non-directory entries are ignored.
/// Result is sorted by name so callers see a stable order.
pub fn discover_accounts(mail_root: &Path) -> Vec<Account> {
    let Ok(entries) = fs::read_dir(mail_root) else {
        return Vec::new();
    };
    let mut accounts: Vec<Account> = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let address = read_address(&entry.path());
        accounts.push(Account { name, address });
    }
    accounts.sort_by(|a, b| a.name.cmp(&b.name));
    accounts
}

/// Read the optional `address` file inside an account directory.
/// Returns `None` if the file is missing, empty, or whitespace-only.
pub fn read_address(account_dir: &Path) -> Option<String> {
    let raw = fs::read_to_string(account_dir.join("address")).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

// ---- maildir reading (pure on a path) ----

fn read_messages(dir: &Path) -> Result<Vec<Summary>> {
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let raw = match fs::read(entry.path()) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let parser = mail_parser::MessageParser::default();
        let Some(msg) = parser.parse(&raw) else {
            continue;
        };

        let from = extract_from(&msg);
        let subject = msg.subject().unwrap_or("(no subject)").to_string();
        let filename = entry
            .file_name()
            .to_str()
            .map(str::to_string)
            .unwrap_or_default();
        out.push(Summary {
            from,
            subject,
            filename,
        });
    }
    Ok(out)
}

fn extract_from(msg: &mail_parser::Message<'_>) -> String {
    use mail_parser::Address;
    match msg.from() {
        Some(Address::List(list)) => list
            .first()
            .and_then(|addr| addr.address.as_deref())
            .unwrap_or("(unknown)")
            .to_string(),
        Some(Address::Group(groups)) => groups
            .first()
            .and_then(|g| g.addresses.first())
            .and_then(|addr| addr.address.as_deref())
            .unwrap_or("(unknown)")
            .to_string(),
        None => "(unknown)".to_string(),
    }
}

fn count_entries(dir: &Path) -> usize {
    fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).count())
        .unwrap_or(0)
}

// ---- report building ----

pub const RECENT_CAP: usize = 10;

pub fn mail_report(accounts: &[Account], mail_root: &Path) -> Result<Vec<AccountReport>> {
    let mut out = Vec::with_capacity(accounts.len());
    for account in accounts {
        let inbox = mail_root.join(&account.name).join("INBOX");
        let new_dir = inbox.join("new");
        let cur_dir = inbox.join("cur");

        let mut unread = read_messages(&new_dir)?;
        unread.sort_by(|a, b| a.from.cmp(&b.from));
        let unread_count = unread.len();
        let read_count = count_entries(&cur_dir);

        let label = match &account.address {
            Some(addr) => format!("{} <{}>", account.name, addr),
            None => account.name.clone(),
        };

        out.push(AccountReport {
            account: account.name.clone(),
            label,
            unread_count,
            read_count,
            recent: unread.into_iter().take(RECENT_CAP).collect(),
        });
    }
    Ok(out)
}

// ---- list ----

/// Where in the maildir a message lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageState {
    /// `INBOX/new/` — unread
    New,
    /// `INBOX/cur/` — read
    Seen,
    /// `Archive/cur/`
    Archived,
}

/// One row in `humos mail ls` output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MessageListing {
    pub state: MessageState,
    pub from: String,
    pub subject: String,
    pub filename: String,
}

/// Filters for [`list_messages`]. Defaults to "INBOX, both states, no limit"
/// — the CLI applies a limit on top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListOptions {
    pub include_unread: bool,
    pub include_read: bool,
    pub include_archive: bool,
    pub limit: Option<usize>,
}

impl Default for ListOptions {
    fn default() -> Self {
        ListOptions {
            include_unread: true,
            include_read: true,
            include_archive: false,
            limit: None,
        }
    }
}

/// List messages under `mail_root/<account>/` matching `opts`. Newest first
/// (by file mtime). Two-phase: stat all candidate files, sort + truncate, then
/// parse headers only for the survivors. Lets `--limit 25` stay snappy on a
/// 10k-message inbox.
pub fn list_messages(
    mail_root: &Path,
    account: &str,
    opts: &ListOptions,
) -> Result<Vec<MessageListing>> {
    if account.is_empty() || account.contains('/') || account.contains("..") {
        anyhow::bail!("invalid account: {account:?}");
    }
    let account_dir = mail_root.join(account);

    struct Meta {
        state: MessageState,
        filename: String,
        mtime: std::time::SystemTime,
        path: PathBuf,
    }

    let mut metas: Vec<Meta> = Vec::new();
    let mut sources: Vec<(PathBuf, MessageState)> = Vec::new();
    if opts.include_unread {
        sources.push((account_dir.join("INBOX").join("new"), MessageState::New));
    }
    if opts.include_read {
        sources.push((account_dir.join("INBOX").join("cur"), MessageState::Seen));
    }
    if opts.include_archive {
        sources.push((account_dir.join("Archive").join("cur"), MessageState::Archived));
    }

    for (dir, state) in sources {
        if !dir.exists() {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            let filename = entry.file_name().to_string_lossy().to_string();
            metas.push(Meta {
                state,
                filename,
                mtime,
                path: entry.path(),
            });
        }
    }

    // Newest first.
    metas.sort_by(|a, b| b.mtime.cmp(&a.mtime));
    if let Some(limit) = opts.limit {
        metas.truncate(limit);
    }

    let parser = mail_parser::MessageParser::default();
    let mut out = Vec::with_capacity(metas.len());
    for m in metas {
        let raw = match fs::read(&m.path) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let Some(msg) = parser.parse(&raw) else {
            continue;
        };
        let from = extract_from(&msg);
        let subject = msg.subject().unwrap_or("(no subject)").to_string();
        out.push(MessageListing {
            state: m.state,
            from,
            subject,
            filename: m.filename,
        });
    }
    Ok(out)
}

// ---- read ----

/// Read the raw bytes of a single message from the maildir. Looks in
/// `INBOX/new`, `INBOX/cur`, then `Archive/cur` in that order. Returns an
/// error if the message is not found, or if the account/filename is suspicious
/// (empty, contains `/`, or `..`).
///
/// Output is the raw on-disk bytes (RFC-822 plus any maildir metadata at the
/// start, which there typically isn't — Maildir embeds metadata in the
/// filename, not the file body). Suitable for piping into another tool.
pub fn read_message(mail_root: &Path, account: &str, filename: &str) -> Result<Vec<u8>> {
    if filename.is_empty() || filename.contains('/') || filename.contains("..") {
        anyhow::bail!("invalid filename: {filename:?}");
    }
    if account.is_empty() || account.contains('/') || account.contains("..") {
        anyhow::bail!("invalid account: {account:?}");
    }

    let account_dir = mail_root.join(account);
    let candidates = [
        account_dir.join("INBOX").join("new").join(filename),
        account_dir.join("INBOX").join("cur").join(filename),
        account_dir.join("Archive").join("cur").join(filename),
    ];

    for path in &candidates {
        if path.is_file() {
            return fs::read(path)
                .with_context(|| format!("reading {}", path.display()));
        }
    }
    anyhow::bail!(
        "message {filename:?} not found in {account}/INBOX/{{new,cur}} or {account}/Archive/cur"
    );
}

// ---- archive ----

/// Move a message out of `INBOX/{new,cur}/` into `Archive/cur/` (creating the
/// destination if necessary). The maildir `:2,S` "seen" suffix is appended if
/// the source filename does not already carry maildir flags, so the archived
/// copy is consistently marked read.
///
/// Returns an error if the message is not found in either `new/` or `cur/`,
/// if the filename is suspicious (empty, contains `/`, or `..`), or if the I/O
/// fails. Refuses to overwrite an existing file at the destination.
pub fn archive_message(mail_root: &Path, account: &str, filename: &str) -> Result<PathBuf> {
    if filename.is_empty() || filename.contains('/') || filename.contains("..") {
        anyhow::bail!("invalid filename: {filename:?}");
    }
    if account.is_empty() || account.contains('/') || account.contains("..") {
        anyhow::bail!("invalid account: {account:?}");
    }

    let account_dir = mail_root.join(account);
    let inbox = account_dir.join("INBOX");
    let new_path = inbox.join("new").join(filename);
    let cur_path = inbox.join("cur").join(filename);

    let src = if new_path.is_file() {
        new_path
    } else if cur_path.is_file() {
        cur_path
    } else {
        anyhow::bail!(
            "message {filename:?} not found in {}/INBOX/{{new,cur}}",
            account
        );
    };

    let archive_cur = account_dir.join("Archive").join("cur");
    fs::create_dir_all(&archive_cur)
        .with_context(|| format!("creating {}", archive_cur.display()))?;

    let dest_name = if filename.contains(":2,") {
        filename.to_string()
    } else {
        format!("{filename}:2,S")
    };
    let dest = archive_cur.join(&dest_name);

    if dest.exists() {
        anyhow::bail!("refusing to overwrite existing archive entry {dest:?}");
    }

    fs::rename(&src, &dest)
        .with_context(|| format!("moving {} -> {}", src.display(), dest.display()))?;
    Ok(dest)
}

// ---- formatting (CLI) ----

pub fn format_report(reports: &[AccountReport]) -> String {
    let mut out = String::new();
    if reports.is_empty() {
        out.push_str(
            "no mail accounts under ~/.humOS/mail/. \
             create one with: mkdir ~/.humOS/mail/<name>\n",
        );
        return out;
    }
    for r in reports {
        out.push_str(&format!(
            "[{}] {} unread, {} read\n",
            r.label, r.unread_count, r.read_count
        ));
        for s in &r.recent {
            out.push_str(&format!(
                "  NEW  {:<32}  {}\n",
                truncate(&s.from, 32),
                s.subject
            ));
        }
    }
    out
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}\u{2026}", truncated)
    }
}

// ================= tests =================
#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &[u8]) -> mail_parser::Message<'_> {
        mail_parser::MessageParser::default().parse(raw).unwrap()
    }

    // ---- truncate ----
    #[test]
    fn truncate_shorter_than_max_is_unchanged() {
        assert_eq!(truncate("hi", 32), "hi");
    }

    #[test]
    fn truncate_equal_to_max_is_unchanged() {
        assert_eq!(truncate("abcde", 5), "abcde");
    }

    #[test]
    fn truncate_longer_than_max_ends_with_ellipsis_and_has_max_chars() {
        let s = "a".repeat(40);
        let t = truncate(&s, 10);
        assert_eq!(t.chars().count(), 10);
        assert!(t.ends_with('\u{2026}'));
    }

    #[test]
    fn truncate_unicode_counts_chars_not_bytes() {
        let s = "日本語テスト";
        assert_eq!(truncate(s, 10), s);
    }

    // ---- extract_from ----
    #[test]
    fn extract_from_bare_address() {
        let msg = parse(b"From: alice@example.com\r\nSubject: Hi\r\n\r\nbody");
        assert_eq!(extract_from(&msg), "alice@example.com");
    }

    #[test]
    fn extract_from_named_address() {
        let msg = parse(b"From: Alice <alice@example.com>\r\nSubject: Hi\r\n\r\nbody");
        assert_eq!(extract_from(&msg), "alice@example.com");
    }

    #[test]
    fn extract_from_missing_header_returns_unknown() {
        let msg = parse(b"Subject: Hi\r\n\r\nbody");
        assert_eq!(extract_from(&msg), "(unknown)");
    }

    // ---- discover_accounts ----
    #[test]
    fn discover_accounts_missing_root_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope");
        assert!(discover_accounts(&missing).is_empty());
    }

    #[test]
    fn discover_accounts_empty_root_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(discover_accounts(tmp.path()).is_empty());
    }

    #[test]
    fn discover_accounts_returns_one_per_subdirectory_sorted_by_name() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["icloud", "gmail", "fastmail"] {
            fs::create_dir_all(tmp.path().join(name)).unwrap();
        }
        let accounts = discover_accounts(tmp.path());
        let names: Vec<_> = accounts.iter().map(|a| a.name.clone()).collect();
        assert_eq!(names, vec!["fastmail", "gmail", "icloud"]);
    }

    #[test]
    fn discover_accounts_account_with_no_address_file_has_none() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("gmail")).unwrap();
        let accounts = discover_accounts(tmp.path());
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].name, "gmail");
        assert_eq!(accounts[0].address, None);
    }

    #[test]
    fn discover_accounts_reads_address_file_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("gmail");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("address"), "felix@gmail.com\n").unwrap();
        let accounts = discover_accounts(tmp.path());
        assert_eq!(accounts[0].address.as_deref(), Some("felix@gmail.com"));
    }

    #[test]
    fn discover_accounts_skips_top_level_files() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("gmail")).unwrap();
        fs::write(tmp.path().join("README"), "stray").unwrap();
        let accounts = discover_accounts(tmp.path());
        let names: Vec<_> = accounts.iter().map(|a| a.name.clone()).collect();
        assert_eq!(names, vec!["gmail"]);
    }

    #[test]
    fn discover_accounts_skips_dotfiles() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("gmail")).unwrap();
        fs::create_dir_all(tmp.path().join(".cache")).unwrap();
        fs::create_dir_all(tmp.path().join(".notmuch")).unwrap();
        let accounts = discover_accounts(tmp.path());
        let names: Vec<_> = accounts.iter().map(|a| a.name.clone()).collect();
        assert_eq!(names, vec!["gmail"]);
    }

    // ---- read_address ----
    #[test]
    fn read_address_missing_file_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(read_address(tmp.path()), None);
    }

    #[test]
    fn read_address_empty_file_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("address"), "").unwrap();
        assert_eq!(read_address(tmp.path()), None);
    }

    #[test]
    fn read_address_whitespace_only_is_none() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("address"), "   \n  \t  \n").unwrap();
        assert_eq!(read_address(tmp.path()), None);
    }

    #[test]
    fn read_address_trims_surrounding_whitespace_and_newlines() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("address"), "  felix@gmail.com  \n").unwrap();
        assert_eq!(
            read_address(tmp.path()).as_deref(),
            Some("felix@gmail.com")
        );
    }

    // ---- mail_report ----
    fn write_maildir_message(dir: &Path, filename: &str, from: &str, subject: &str) {
        fs::create_dir_all(dir).unwrap();
        let body = format!("From: {}\r\nSubject: {}\r\n\r\nbody", from, subject);
        fs::write(dir.join(filename), body).unwrap();
    }

    #[test]
    fn mail_report_counts_unread_reads_and_lists_summaries_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        let gmail_new = root.join("gmail").join("INBOX").join("new");
        let gmail_cur = root.join("gmail").join("INBOX").join("cur");
        write_maildir_message(&gmail_new, "1", "bob@example.com", "Bob says hi");
        write_maildir_message(&gmail_new, "2", "alice@example.com", "Alice says hi");
        write_maildir_message(&gmail_cur, "3:2,S", "carol@example.com", "Already read");

        let accounts = vec![Account {
            name: "gmail".into(),
            address: Some("me@gmail.com".into()),
        }];

        let reports = mail_report(&accounts, root).unwrap();
        assert_eq!(reports.len(), 1);

        let r = &reports[0];
        assert_eq!(r.account, "gmail");
        assert_eq!(r.label, "gmail <me@gmail.com>");
        assert_eq!(r.unread_count, 2);
        assert_eq!(r.read_count, 1);
        let froms: Vec<_> = r.recent.iter().map(|s| s.from.as_str()).collect();
        assert_eq!(froms, vec!["alice@example.com", "bob@example.com"]);
    }

    #[test]
    fn mail_report_handles_account_with_no_maildir_yet() {
        let tmp = tempfile::tempdir().unwrap();
        let accounts = vec![Account {
            name: "fresh".into(),
            address: None,
        }];
        let reports = mail_report(&accounts, tmp.path()).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].label, "fresh");
        assert_eq!(reports[0].unread_count, 0);
        assert_eq!(reports[0].read_count, 0);
        assert!(reports[0].recent.is_empty());
    }

    #[test]
    fn mail_report_omits_address_in_label_when_none() {
        let tmp = tempfile::tempdir().unwrap();
        let accounts = vec![Account {
            name: "gmail".into(),
            address: None,
        }];
        let reports = mail_report(&accounts, tmp.path()).unwrap();
        assert_eq!(reports[0].label, "gmail");
    }

    #[test]
    fn mail_report_caps_recent_at_ten() {
        let tmp = tempfile::tempdir().unwrap();
        let new_dir = tmp.path().join("a").join("INBOX").join("new");
        for i in 0..15 {
            write_maildir_message(
                &new_dir,
                &format!("msg{:02}", i),
                &format!("u{:02}@example.com", i),
                &format!("Subject {}", i),
            );
        }
        let accounts = vec![Account {
            name: "a".into(),
            address: None,
        }];
        let reports = mail_report(&accounts, tmp.path()).unwrap();
        assert_eq!(reports[0].unread_count, 15);
        assert_eq!(reports[0].recent.len(), RECENT_CAP);
    }

    // ---- format_report ----
    #[test]
    fn format_report_empty_points_user_at_the_filesystem() {
        let out = format_report(&[]);
        assert!(out.contains("~/.humOS/mail/"));
        assert!(out.contains("mkdir"));
    }

    #[test]
    fn format_report_renders_header_and_messages() {
        let report = AccountReport {
            account: "gmail".into(),
            label: "gmail".to_string(),
            unread_count: 2,
            read_count: 10,
            recent: vec![
                Summary {
                    from: "alice@example.com".into(),
                    subject: "Hi".into(),
                    filename: "1".into(),
                },
                Summary {
                    from: "bob@example.com".into(),
                    subject: "Bye".into(),
                    filename: "2".into(),
                },
            ],
        };
        let out = format_report(&[report]);
        assert!(out.contains("[gmail] 2 unread, 10 read"));
        assert!(out.contains("alice@example.com"));
        assert!(out.contains("Hi"));
        assert!(out.contains("bob@example.com"));
        assert!(out.contains("Bye"));
    }

    // ---- list_messages ----
    fn touch_with_mtime(dir: &Path, name: &str, from: &str, subject: &str, mtime_secs: u64) {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        let body = format!("From: {from}\r\nSubject: {subject}\r\n\r\nbody");
        fs::write(&path, body).unwrap();
        let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(mtime_secs);
        filetime::set_file_mtime(&path, filetime::FileTime::from_system_time(when)).unwrap();
    }

    #[test]
    fn list_messages_returns_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let new_dir = tmp.path().join("gmail").join("INBOX").join("new");
        touch_with_mtime(&new_dir, "old", "alice@x", "Old", 1_000_000_000);
        touch_with_mtime(&new_dir, "mid", "bob@x", "Mid", 1_500_000_000);
        touch_with_mtime(&new_dir, "newest", "carol@x", "Newest", 2_000_000_000);

        let out = list_messages(tmp.path(), "gmail", &ListOptions::default()).unwrap();
        let subjects: Vec<_> = out.iter().map(|m| m.subject.as_str()).collect();
        assert_eq!(subjects, vec!["Newest", "Mid", "Old"]);
    }

    #[test]
    fn list_messages_default_includes_unread_and_read_skips_archive() {
        let tmp = tempfile::tempdir().unwrap();
        let acct = tmp.path().join("gmail");
        touch_with_mtime(&acct.join("INBOX").join("new"), "u", "a@x", "Unread", 100);
        touch_with_mtime(&acct.join("INBOX").join("cur"), "r:2,S", "b@x", "Read", 200);
        touch_with_mtime(&acct.join("Archive").join("cur"), "a:2,S", "c@x", "Archived", 300);

        let out = list_messages(tmp.path(), "gmail", &ListOptions::default()).unwrap();
        let subjects: Vec<_> = out.iter().map(|m| m.subject.as_str()).collect();
        assert_eq!(subjects, vec!["Read", "Unread"]); // newer mtime first; archive excluded
    }

    #[test]
    fn list_messages_with_unread_only() {
        let tmp = tempfile::tempdir().unwrap();
        let acct = tmp.path().join("gmail");
        touch_with_mtime(&acct.join("INBOX").join("new"), "u", "a@x", "Unread", 100);
        touch_with_mtime(&acct.join("INBOX").join("cur"), "r:2,S", "b@x", "Read", 200);

        let opts = ListOptions {
            include_read: false,
            ..Default::default()
        };
        let out = list_messages(tmp.path(), "gmail", &opts).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].state, MessageState::New);
        assert_eq!(out[0].subject, "Unread");
    }

    #[test]
    fn list_messages_with_archive_only() {
        let tmp = tempfile::tempdir().unwrap();
        let acct = tmp.path().join("gmail");
        touch_with_mtime(&acct.join("INBOX").join("new"), "u", "a@x", "Unread", 100);
        touch_with_mtime(&acct.join("Archive").join("cur"), "a:2,S", "c@x", "Archived", 300);

        let opts = ListOptions {
            include_unread: false,
            include_read: false,
            include_archive: true,
            ..Default::default()
        };
        let out = list_messages(tmp.path(), "gmail", &opts).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].state, MessageState::Archived);
        assert_eq!(out[0].subject, "Archived");
    }

    #[test]
    fn list_messages_respects_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let new_dir = tmp.path().join("gmail").join("INBOX").join("new");
        for i in 0..10 {
            touch_with_mtime(&new_dir, &format!("m{i:02}"), "x@x", &format!("S{i}"), 1000 + i);
        }
        let opts = ListOptions {
            limit: Some(3),
            ..Default::default()
        };
        let out = list_messages(tmp.path(), "gmail", &opts).unwrap();
        assert_eq!(out.len(), 3);
        // Newest 3 = mtime 1009, 1008, 1007 = subjects S9, S8, S7
        let subjects: Vec<_> = out.iter().map(|m| m.subject.as_str()).collect();
        assert_eq!(subjects, vec!["S9", "S8", "S7"]);
    }

    #[test]
    fn list_messages_empty_account_dir_is_ok() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("gmail")).unwrap();
        let out = list_messages(tmp.path(), "gmail", &ListOptions::default()).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn list_messages_rejects_path_traversal() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(list_messages(tmp.path(), "..", &ListOptions::default()).is_err());
        assert!(list_messages(tmp.path(), "a/b", &ListOptions::default()).is_err());
        assert!(list_messages(tmp.path(), "", &ListOptions::default()).is_err());
    }

    // ---- read_message ----
    #[test]
    fn read_message_returns_bytes_from_inbox_new() {
        let tmp = tempfile::tempdir().unwrap();
        let new_dir = tmp.path().join("gmail").join("INBOX").join("new");
        fs::create_dir_all(&new_dir).unwrap();
        let body = b"From: alice@example.com\r\nSubject: Hi\r\n\r\nbody bytes";
        fs::write(new_dir.join("abc"), body).unwrap();

        let got = read_message(tmp.path(), "gmail", "abc").unwrap();
        assert_eq!(got, body);
    }

    #[test]
    fn read_message_falls_through_to_inbox_cur() {
        let tmp = tempfile::tempdir().unwrap();
        let cur_dir = tmp.path().join("gmail").join("INBOX").join("cur");
        fs::create_dir_all(&cur_dir).unwrap();
        let body = b"From: x@y\r\nSubject: cur\r\n\r\nin cur";
        fs::write(cur_dir.join("xyz:2,S"), body).unwrap();

        let got = read_message(tmp.path(), "gmail", "xyz:2,S").unwrap();
        assert_eq!(got, body);
    }

    #[test]
    fn read_message_falls_through_to_archive_cur() {
        let tmp = tempfile::tempdir().unwrap();
        let arc_dir = tmp.path().join("gmail").join("Archive").join("cur");
        fs::create_dir_all(&arc_dir).unwrap();
        let body = b"From: x@y\r\nSubject: archived\r\n\r\nin archive";
        fs::write(arc_dir.join("old:2,S"), body).unwrap();

        let got = read_message(tmp.path(), "gmail", "old:2,S").unwrap();
        assert_eq!(got, body);
    }

    #[test]
    fn read_message_missing_file_errors() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("gmail").join("INBOX").join("new")).unwrap();
        let err = read_message(tmp.path(), "gmail", "nope").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn read_message_rejects_path_traversal_in_filename() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_message(tmp.path(), "gmail", "../../etc/passwd").is_err());
        assert!(read_message(tmp.path(), "gmail", "a/b").is_err());
        assert!(read_message(tmp.path(), "gmail", "").is_err());
    }

    #[test]
    fn read_message_rejects_path_traversal_in_account() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_message(tmp.path(), "..", "abc").is_err());
        assert!(read_message(tmp.path(), "a/b", "abc").is_err());
        assert!(read_message(tmp.path(), "", "abc").is_err());
    }

    #[test]
    fn read_message_preserves_raw_bytes_including_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let new_dir = tmp.path().join("gmail").join("INBOX").join("new");
        fs::create_dir_all(&new_dir).unwrap();
        // Include some non-UTF-8 bytes to confirm we return raw &[u8], not a
        // String that would lossy-replace them.
        let body: Vec<u8> = vec![0xff, 0xfe, b'\r', b'\n', b'h', b'i'];
        fs::write(new_dir.join("bin"), &body).unwrap();
        assert_eq!(read_message(tmp.path(), "gmail", "bin").unwrap(), body);
    }

    // ---- archive_message ----
    #[test]
    fn archive_message_moves_from_new_into_archive_cur_with_seen_flag() {
        let tmp = tempfile::tempdir().unwrap();
        let new_dir = tmp.path().join("gmail").join("INBOX").join("new");
        write_maildir_message(&new_dir, "abc", "alice@example.com", "Hi");

        let dest = archive_message(tmp.path(), "gmail", "abc").unwrap();
        assert!(dest.ends_with("Archive/cur/abc:2,S"));
        assert!(dest.is_file());
        assert!(!new_dir.join("abc").exists());
    }

    #[test]
    fn archive_message_moves_from_cur_preserving_existing_flags() {
        let tmp = tempfile::tempdir().unwrap();
        let cur_dir = tmp.path().join("gmail").join("INBOX").join("cur");
        write_maildir_message(&cur_dir, "xyz:2,S", "alice@example.com", "Hi");

        let dest = archive_message(tmp.path(), "gmail", "xyz:2,S").unwrap();
        assert!(dest.ends_with("Archive/cur/xyz:2,S"));
        assert!(!cur_dir.join("xyz:2,S").exists());
    }

    #[test]
    fn archive_message_missing_message_errors() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("gmail").join("INBOX").join("new")).unwrap();
        let err = archive_message(tmp.path(), "gmail", "nope").unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn archive_message_rejects_path_traversal_in_filename() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(archive_message(tmp.path(), "gmail", "../../etc/passwd").is_err());
        assert!(archive_message(tmp.path(), "gmail", "a/b").is_err());
        assert!(archive_message(tmp.path(), "gmail", "").is_err());
    }

    #[test]
    fn archive_message_rejects_path_traversal_in_account() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(archive_message(tmp.path(), "..", "abc").is_err());
        assert!(archive_message(tmp.path(), "a/b", "abc").is_err());
        assert!(archive_message(tmp.path(), "", "abc").is_err());
    }

    #[test]
    fn archive_message_refuses_to_overwrite_existing_destination() {
        let tmp = tempfile::tempdir().unwrap();
        let new_dir = tmp.path().join("gmail").join("INBOX").join("new");
        write_maildir_message(&new_dir, "abc", "a@x", "hi");
        let archive_cur = tmp.path().join("gmail").join("Archive").join("cur");
        fs::create_dir_all(&archive_cur).unwrap();
        fs::write(archive_cur.join("abc:2,S"), "preexisting").unwrap();

        let err = archive_message(tmp.path(), "gmail", "abc").unwrap_err();
        assert!(err.to_string().contains("refusing to overwrite"));
        // source untouched
        assert!(new_dir.join("abc").exists());
    }
}
