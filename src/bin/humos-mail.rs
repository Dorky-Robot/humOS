//! humos-mail — mail operations over `~/.humOS/mail/`.
//!
//! Subcommands:
//!   (none) | report            unread/read counts per account
//!   ls [account] [flags]       list messages (newest first)
//!   cat [account] FILENAME     print RFC-822 of one message to stdout
//!   fetch [account]            pull via mbsync
//!   send [account]             read RFC-822 on stdin, send via himalaya
//!   triage [account]           classify unread via local LLM
//!   account [list]             list discovered accounts
//!   account add NAME EMAIL     create a new account (password from stdin)
//!   account remove NAME        delete account dir + keyring entry

use anyhow::{Context, Result};
use humos::account::{auto_provider_config, create_account, delete_account};
use humos::cli::{self, Output};
use humos::himalaya::{self, FromAccount};
use humos::mail::{
    Account, ListOptions, MessageListing, MessageState, discover_accounts, format_report,
    list_messages, mail_report, read_message, truncate,
};
use humos::mbsync::{self, Target};
use humos::shell::SystemRunner;
use humos::triage::triage_account;
use std::io::{self, BufRead, IsTerminal, Read, Write};
// BufRead is needed for StdinLock::read_line (trait in scope).
use std::process::ExitCode;

const USAGE: &str = "\
usage: humos-mail [<subcommand>] [options]

Subcommands:
  report                       unread/read counts (default)
  ls [account] [flags]         list messages (newest first)
  cat [account] FILENAME       print RFC-822 of one message to stdout
  fetch [account]              pull mail via mbsync
  send [account]               send RFC-822 message from stdin via himalaya
  triage [account]             classify unread mail with a local LLM
  account [list]               list accounts
  account add NAME EMAIL       create a new account (password from stdin)
  account remove NAME          delete account dir + keyring entry

Global flags:
  --json   emit machine-readable envelope (report / ls / account list)
  --help   show this message
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (sub, rest) = split_subcommand(&args);
    match sub.as_deref() {
        None => {
            if wants_help(&rest) {
                println!("{USAGE}");
                ExitCode::SUCCESS
            } else {
                run_report(rest)
            }
        }
        Some("report") => run_report(rest),
        Some("ls") => run_ls(rest),
        Some("cat") => run_cat(rest),
        Some("fetch") => run_fetch(rest),
        Some("send") => run_send(rest),
        Some("triage") => run_triage(rest),
        Some("account") => run_account(rest),
        Some(other) => {
            eprintln!("humos-mail: unknown subcommand {other:?}\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn split_subcommand(args: &[String]) -> (Option<String>, Vec<String>) {
    const VERBS: &[&str] = &[
        "report", "ls", "cat", "fetch", "send", "triage", "account",
    ];
    if let Some(first) = args.first() {
        if VERBS.contains(&first.as_str()) {
            return (Some(first.clone()), args[1..].to_vec());
        }
    }
    (None, args.to_vec())
}

fn wants_help(args: &[String]) -> bool {
    args.iter().any(|a| a == "--help" || a == "-h")
}

fn first_positional(args: &[String]) -> Option<String> {
    args.iter().find(|a| !a.starts_with('-')).cloned()
}

// ---- report (default) ----

fn run_report(args: Vec<String>) -> ExitCode {
    cli::run(args, USAGE, |_flags| {
        let mail_root = humos::humos_dir()?.join("mail");
        let accounts = discover_accounts(&mail_root);
        let data = mail_report(&accounts, &mail_root)?;
        let human = format_report(&data);
        Ok(Output { data, human })
    })
}

// ---- fetch ----

fn run_fetch(args: Vec<String>) -> ExitCode {
    if wants_help(&args) {
        println!(
            "usage: humos-mail fetch [account]\n\n\
             Pull new mail from IMAP into ~/.humOS/mail/<account>/INBOX/ by\n\
             generating a .mbsyncrc from per-account config and invoking mbsync.\n\
             With no argument, fetches every account that has IMAP config.\n"
        );
        return ExitCode::SUCCESS;
    }
    let target = match first_positional(&args) {
        Some(name) => Target::One(name),
        None => Target::All,
    };
    let humos_root = match humos::humos_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("error: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    match mbsync::fetch(&humos_root, &target, &SystemRunner) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(code) => {
            eprintln!("mbsync exited {code}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

// ---- ls ----

const LS_USAGE: &str = "\
usage: humos-mail ls [account] [flags]

List messages, newest first. Default: INBOX (unread + read), 25 most recent.

Filters (mutually exclusive — last wins):
  --unread     only INBOX/new/
  --archive    only Archive/cur/

Limit:
  --limit N    cap at N messages (default 25)
  --all        no limit

Output (text mode, columns are state | from | subject | filename):
  NEW   alice@example.com         Hello                            abc:2,S
  SEEN  bob@example.com           Re: status                       xyz:2,S

Pipe-friendly:
  humos mail ls --unread --json | jq -r '.data[].filename'
  humos mail ls --json | jq -r '.data[] | select(.from|test(\"@github.com\"))'
";

fn run_ls(args: Vec<String>) -> ExitCode {
    if wants_help(&args) {
        println!("{LS_USAGE}");
        return ExitCode::SUCCESS;
    }
    let (account, opts) = match parse_ls_args(&args) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };
    cli::run(args, LS_USAGE, |_flags| {
        let mail_root = humos::humos_dir()?.join("mail");
        let account = resolve_account(account.clone(), &mail_root)?;
        let data = list_messages(&mail_root, &account, &opts)?;
        let human = format_listing(&data);
        Ok(Output { data, human })
    })
}

fn parse_ls_args(args: &[String]) -> Result<(Option<String>, ListOptions)> {
    let mut opts = ListOptions {
        limit: Some(25),
        ..Default::default()
    };
    let mut account: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        match a.as_str() {
            "--unread" => {
                opts.include_unread = true;
                opts.include_read = false;
                opts.include_archive = false;
            }
            "--archive" => {
                opts.include_unread = false;
                opts.include_read = false;
                opts.include_archive = true;
            }
            "--all" => {
                opts.limit = None;
            }
            "--limit" => {
                i += 1;
                let n: usize = args
                    .get(i)
                    .ok_or_else(|| anyhow::anyhow!("--limit needs a number"))?
                    .parse()
                    .map_err(|_| anyhow::anyhow!("--limit needs a positive integer"))?;
                opts.limit = Some(n);
            }
            "--json" | "--help" | "-h" => {} // handled by cli::run
            s if s.starts_with('-') => anyhow::bail!("unknown flag: {s}"),
            s => {
                if account.is_some() {
                    anyhow::bail!("unexpected positional arg {s:?}");
                }
                account = Some(s.to_string());
            }
        }
        i += 1;
    }
    Ok((account, opts))
}

fn resolve_account(explicit: Option<String>, mail_root: &std::path::Path) -> Result<String> {
    if let Some(name) = explicit {
        return Ok(name);
    }
    let accounts = discover_accounts(mail_root);
    match accounts.as_slice() {
        [single] => Ok(single.name.clone()),
        [] => anyhow::bail!("no accounts under ~/.humOS/mail/"),
        _ => anyhow::bail!(
            "multiple accounts configured ({}); pass account name as first arg",
            accounts.len()
        ),
    }
}

fn format_listing(rows: &[MessageListing]) -> String {
    if rows.is_empty() {
        return "no messages\n".to_string();
    }
    let mut out = String::new();
    for r in rows {
        let state = match r.state {
            MessageState::New => "NEW ",
            MessageState::Seen => "SEEN",
            MessageState::Archived => "ARCH",
        };
        out.push_str(&format!(
            "{}  {:<32}  {:<48}  {}\n",
            state,
            truncate(&r.from, 32),
            truncate(&r.subject, 48),
            r.filename
        ));
    }
    out
}

// ---- cat ----

fn run_cat(args: Vec<String>) -> ExitCode {
    if wants_help(&args) {
        println!(
            "usage: humos-mail cat [account] FILENAME\n\n\
             Print the raw RFC-822 bytes of one message to stdout. Looks in\n\
             INBOX/new, INBOX/cur, then Archive/cur. With no account argument,\n\
             the single configured account is used (errors if there are several).\n\n\
             Pipe-friendly:\n\n  \
               humos mail cat $ID | llm 'draft a reply' | humos mail send\n  \
               humos mail ls --unread --json | jq -r '.data[0].filename' | xargs humos mail cat\n"
        );
        return ExitCode::SUCCESS;
    }
    let positional: Vec<String> = args.into_iter().filter(|a| !a.starts_with('-')).collect();
    let (account, filename) = match positional.as_slice() {
        [filename] => (None, filename.clone()),
        [account, filename] => (Some(account.clone()), filename.clone()),
        _ => {
            eprintln!("usage: humos-mail cat [account] FILENAME");
            return ExitCode::FAILURE;
        }
    };

    match do_cat(account, &filename) {
        Ok(bytes) => {
            if let Err(e) = io::stdout().write_all(&bytes) {
                if e.kind() != io::ErrorKind::BrokenPipe {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn do_cat(account: Option<String>, filename: &str) -> Result<Vec<u8>> {
    let mail_root = humos::humos_dir()?.join("mail");
    let account = resolve_account(account, &mail_root)?;
    read_message(&mail_root, &account, filename)
}

// ---- send ----

fn run_send(args: Vec<String>) -> ExitCode {
    if wants_help(&args) {
        println!(
            "usage: humos-mail send [account] < message.eml\n\n\
             Reads a complete RFC-822 message on stdin and sends it via himalaya.\n\
             With no account argument, uses the first configured account.\n\
             No prompts: the caller composes the entire message (From, To,\n\
             Subject, body) and pipes it in.\n"
        );
        return ExitCode::SUCCESS;
    }
    match do_send(first_positional(&args)) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(code) => {
            eprintln!("himalaya exited {code}");
            ExitCode::FAILURE
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn do_send(account: Option<String>) -> Result<i32> {
    let stdin = io::stdin();
    if stdin.is_terminal() {
        anyhow::bail!(
            "humos-mail send expects an RFC-822 message on stdin, not a TTY.\n\
             example: printf 'From: me\\r\\nTo: you\\r\\nSubject: hi\\r\\n\\r\\nbody\\r\\n' \
             | humos-mail send"
        );
    }
    let mut body = Vec::new();
    stdin
        .lock()
        .read_to_end(&mut body)
        .context("reading message from stdin")?;
    if body.is_empty() {
        anyhow::bail!("message body on stdin was empty");
    }
    let from = match account {
        Some(name) => FromAccount::Named(name),
        None => FromAccount::Default,
    };
    himalaya::send(&humos::humos_dir()?, &from, body, &SystemRunner)
}

// ---- triage ----

fn run_triage(args: Vec<String>) -> ExitCode {
    if wants_help(&args) {
        println!(
            "usage: humos-mail triage [account] [--model NAME]\n\n\
             Classify unread mail using a local Ollama model. Writes labels\n\
             into per-message metadata under ~/.humOS/mail/<account>/INBOX/.\n"
        );
        return ExitCode::SUCCESS;
    }
    let mut model: Option<String> = None;
    let mut target: Option<String> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--model" => {
                i += 1;
                model = args.get(i).cloned();
            }
            s if !s.starts_with('-') => target = Some(s.to_string()),
            _ => {}
        }
        i += 1;
    }

    match do_triage(target.as_deref(), model.as_deref()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn do_triage(target: Option<&str>, model: Option<&str>) -> Result<()> {
    let mail_root = humos::humos_dir()?.join("mail");
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
            Ok(n) => eprintln!("{n} message(s) classified"),
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

// ---- account ----

fn run_account(args: Vec<String>) -> ExitCode {
    let (verb, rest) = split_account_verb(&args);
    match verb.as_deref() {
        None | Some("list") => run_account_list(rest),
        Some("add") => run_account_add(rest),
        Some("remove") => run_account_remove(rest),
        Some(other) => {
            eprintln!("humos-mail account: unknown verb {other:?}");
            eprintln!("verbs: list, add, remove");
            ExitCode::FAILURE
        }
    }
}

fn split_account_verb(args: &[String]) -> (Option<String>, Vec<String>) {
    const VERBS: &[&str] = &["list", "add", "remove"];
    if let Some(first) = args.first() {
        if VERBS.contains(&first.as_str()) {
            return (Some(first.clone()), args[1..].to_vec());
        }
    }
    (None, args.to_vec())
}

fn run_account_list(args: Vec<String>) -> ExitCode {
    const LIST_USAGE: &str = "\
usage: humos-mail account [list] [--json]

List accounts discovered under ~/.humOS/mail/.
";
    cli::run(args, LIST_USAGE, |_flags| {
        let mail_root = humos::humos_dir()?.join("mail");
        let data = discover_accounts(&mail_root);
        let human = format_account_list(&data);
        Ok(Output { data, human })
    })
}

fn format_account_list(accounts: &[Account]) -> String {
    if accounts.is_empty() {
        return "no accounts under ~/.humOS/mail/. \
                create one with: humos-mail account add NAME EMAIL\n"
            .to_string();
    }
    let mut out = String::new();
    for a in accounts {
        match &a.address {
            Some(addr) => out.push_str(&format!("{}  <{}>\n", a.name, addr)),
            None => out.push_str(&format!("{}\n", a.name)),
        }
    }
    out
}

fn run_account_add(args: Vec<String>) -> ExitCode {
    if wants_help(&args) {
        println!(
            "usage: humos-mail account add NAME EMAIL\n\n\
             Creates ~/.humOS/mail/<NAME>/ with maildir + IMAP/SMTP config.\n\
             Password is read from stdin (one line, trimmed) and stored in\n\
             the OS keyring under service=\"humos\", account=<NAME>.\n\n\
             Example:\n  \
               echo -n \"$PASS\" | humos-mail account add gmail felix@gmail.com\n"
        );
        return ExitCode::SUCCESS;
    }
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let [name, email] = match positional.as_slice() {
        [n, e] => [n.as_str(), e.as_str()],
        _ => {
            eprintln!("usage: humos-mail account add NAME EMAIL");
            return ExitCode::FAILURE;
        }
    };

    match do_account_add(name, email) {
        Ok(path) => {
            println!("created account {name} at {}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn do_account_add(name: &str, email: &str) -> Result<std::path::PathBuf> {
    let provider = auto_provider_config(email).with_context(|| {
        format!(
            "unknown email provider for {email} — add imap.host/port files by hand \
             or extend auto_provider_config"
        )
    })?;

    let stdin = io::stdin();
    if stdin.is_terminal() {
        eprint!("Password for {email}: ");
        let _ = io::stderr().flush();
    }
    let mut password = String::new();
    stdin
        .lock()
        .read_line(&mut password)
        .context("reading password from stdin")?;
    let password = password.trim();
    if password.is_empty() {
        anyhow::bail!("password cannot be empty");
    }

    let mail_root = humos::humos_dir()?.join("mail");
    create_account(&mail_root, name, email, password, &provider)
}

fn run_account_remove(args: Vec<String>) -> ExitCode {
    if wants_help(&args) {
        println!(
            "usage: humos-mail account remove NAME\n\n\
             Deletes ~/.humOS/mail/<NAME>/ and removes the keyring entry.\n\
             No confirmation prompt — the caller is explicit.\n"
        );
        return ExitCode::SUCCESS;
    }
    let Some(name) = first_positional(&args) else {
        eprintln!("usage: humos-mail account remove NAME");
        return ExitCode::FAILURE;
    };

    let mail_root = match humos::humos_dir() {
        Ok(d) => d.join("mail"),
        Err(e) => {
            eprintln!("error: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    match delete_account(&mail_root, &name) {
        Ok(()) => {
            println!("removed account {name}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

