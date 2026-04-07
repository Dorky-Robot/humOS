//! humos-mail — read mail across the accounts under ~/.humOS/mail/.
//!
//! humOS treats `~/.humOS/` as a file-based DB: each mail account is a
//! directory under `~/.humOS/mail/`, not an entry in a config file. This
//! binary scans that directory at runtime; there is no `config.toml`.
//!
//! Layout (per the vision doc):
//!
//! ```text
//! ~/.humOS/mail/
//!   gmail/
//!     address       ← optional, single line, display only
//!     INBOX/
//!       new/  cur/  tmp/
//!   icloud/
//!     ...
//! ```
//!
//! `humos-mail` does not speak IMAP. `mbsync` (or whatever sync tool the
//! user prefers) is responsible for putting mail under `INBOX/`.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

// ---- entity model ----

/// One mail account discovered on disk under `~/.humOS/mail/`.
#[derive(Debug, PartialEq, Eq)]
struct Account {
    /// Directory name (e.g. "gmail").
    name: String,
    /// Contents of `<account_dir>/address`, trimmed; `None` if absent or empty.
    address: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct Summary {
    from: String,
    subject: String,
}

#[derive(Debug, PartialEq, Eq)]
struct AccountReport {
    label: String,
    unread_count: usize,
    read_count: usize,
    recent: Vec<Summary>,
}

// ---- path helpers (pure) ----

fn humos_dir_for(home: &Path) -> PathBuf {
    home.join(".humOS")
}

fn humos_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("$HOME is not set")?;
    Ok(humos_dir_for(Path::new(&home)))
}

// ---- account discovery (pure on a path) ----

/// Scan `mail_root` for account directories. Each subdirectory is one account.
/// Hidden directories (starting with `.`) and non-directory entries are ignored.
/// Result is sorted by name so callers see a stable order.
fn discover_accounts(mail_root: &Path) -> Vec<Account> {
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
fn read_address(account_dir: &Path) -> Option<String> {
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
        out.push(Summary { from, subject });
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

const RECENT_CAP: usize = 10;

fn mail_report(accounts: &[Account], mail_root: &Path) -> Result<Vec<AccountReport>> {
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
            label,
            unread_count,
            read_count,
            recent: unread.into_iter().take(RECENT_CAP).collect(),
        });
    }
    Ok(out)
}

// ---- formatting ----

fn format_report(reports: &[AccountReport]) -> String {
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

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}\u{2026}", truncated)
    }
}

// ---- main ----

fn main() -> Result<()> {
    let dir = humos_dir()?;
    let mail_root = dir.join("mail");
    let accounts = discover_accounts(&mail_root);
    let reports = mail_report(&accounts, &mail_root)?;
    print!("{}", format_report(&reports));
    Ok(())
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

    // ---- humos_dir_for ----
    #[test]
    fn humos_dir_for_joins_dot_hum_os_onto_home() {
        let home = Path::new("/tmp/fake-home");
        assert_eq!(humos_dir_for(home), PathBuf::from("/tmp/fake-home/.humOS"));
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
        assert_eq!(r.label, "gmail <me@gmail.com>");
        assert_eq!(r.unread_count, 2);
        assert_eq!(r.read_count, 1);
        assert_eq!(
            r.recent,
            vec![
                Summary {
                    from: "alice@example.com".into(),
                    subject: "Alice says hi".into(),
                },
                Summary {
                    from: "bob@example.com".into(),
                    subject: "Bob says hi".into(),
                },
            ]
        );
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
            label: "gmail".to_string(),
            unread_count: 2,
            read_count: 10,
            recent: vec![
                Summary {
                    from: "alice@example.com".into(),
                    subject: "Hi".into(),
                },
                Summary {
                    from: "bob@example.com".into(),
                    subject: "Bye".into(),
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
}
