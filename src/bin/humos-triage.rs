//! humos-triage — classify mail using a local Ollama model.
//!
//! Usage:
//!   humos-triage                     # triage all accounts
//!   humos-triage <account>           # triage one account
//!   humos-triage --model gemma4:31b  # specify model

use anyhow::Result;
use humos::mail::discover_accounts;
use humos::triage::triage_account;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut model: Option<&str> = None;
    let mut target: Option<&str> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--model" => {
                i += 1;
                model = args.get(i).map(|s| s.as_str());
            }
            name => target = Some(name),
        }
        i += 1;
    }

    let dir = humos::humos_dir()?;
    let mail_root = dir.join("mail");
    let accounts = discover_accounts(&mail_root);

    if accounts.is_empty() {
        eprintln!("no accounts under ~/.humOS/mail/");
        return Ok(());
    }

    for account in &accounts {
        if let Some(name) = target {
            if account.name != name {
                continue;
            }
        }

        let account_dir = mail_root.join(&account.name);
        eprint!("triaging {}... ", account.name);
        match triage_account(&account_dir, model) {
            Ok(0) => eprintln!("nothing to triage"),
            Ok(n) => eprintln!(" {n} message(s) classified"),
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
