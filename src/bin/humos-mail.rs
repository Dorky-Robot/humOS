//! humos-mail — print a one-shot text report of mail under `~/.humOS/mail/`.
//!
//! All logic lives in the `humos::mail` library module so other binaries
//! (notably humos-web) can share it.

use anyhow::Result;
use humos::mail::{discover_accounts, format_report, mail_report};

fn main() -> Result<()> {
    let dir = humos::humos_dir()?;
    let mail_root = dir.join("mail");
    let accounts = discover_accounts(&mail_root);
    let reports = mail_report(&accounts, &mail_root)?;
    print!("{}", format_report(&reports));
    Ok(())
}
