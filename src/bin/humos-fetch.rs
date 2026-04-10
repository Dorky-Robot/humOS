//! humos-fetch — pull mail from IMAP into `~/.humOS/mail/<acct>/INBOX/`.
//!
//! Usage:
//!   humos-fetch           # fetch all configured accounts
//!   humos-fetch <name>    # fetch one account

use anyhow::{Context, Result};
use humos::account::{is_fetchable, read_imap_config};
use humos::imap_client::fetch_inbox;
use humos::mail::discover_accounts;

fn main() -> Result<()> {
    let dir = humos::humos_dir()?;
    let mail_root = dir.join("mail");
    let target = std::env::args().nth(1);

    let accounts = discover_accounts(&mail_root);
    if accounts.is_empty() {
        eprintln!("no accounts under ~/.humOS/mail/");
        return Ok(());
    }

    for account in &accounts {
        if let Some(ref name) = target {
            if &account.name != name {
                continue;
            }
        }

        let account_dir = mail_root.join(&account.name);
        if !is_fetchable(&account_dir) {
            if target.is_some() {
                anyhow::bail!(
                    "account {:?} is not configured for IMAP (missing imap.host or address)",
                    account.name
                );
            }
            continue;
        }

        eprint!("fetching {}... ", account.name);
        let config = read_imap_config(&account_dir, &account.name)
            .with_context(|| format!("reading config for {}", account.name))?;
        match fetch_inbox(&config, &account_dir) {
            Ok(count) => eprintln!("{count} new message(s)"),
            Err(e) => eprintln!("error: {e:#}"),
        }
    }

    if let Some(name) = target {
        if !accounts.iter().any(|a| a.name == name) {
            anyhow::bail!("account {name:?} not found");
        }
    }

    Ok(())
}
