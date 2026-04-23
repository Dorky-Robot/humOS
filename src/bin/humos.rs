//! humos — dispatcher for `humos-*` subcommands.
//!
//! `humos <noun> <args...>` execs the binary named `humos-<noun>` with
//! `<args...>` — the same trick `git` uses. Each subcommand binary parses
//! its own verbs and flags; this dispatcher never interprets them.

use std::collections::BTreeSet;
use std::env;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

const USAGE: &str = "\
usage: humos <subcommand> [args...]
       humos                    list discovered subcommands
       humos help <subcommand>  show help for a subcommand

Examples:
  humos mail                         report mail accounts
  humos mail fetch                   pull new mail
  humos mail triage                  classify unread mail
  humos mail accounts list           list accounts
  humos mail accounts add NAME EMAIL create an account
";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(first) = args.first() else {
        return list_subcommands();
    };

    match first.as_str() {
        "--help" | "-h" => {
            print!("{USAGE}");
            print_discovered();
            ExitCode::SUCCESS
        }
        "help" => match args.get(1) {
            Some(sub) => exec_sub(sub, &["--help".to_string()]),
            None => {
                print!("{USAGE}");
                print_discovered();
                ExitCode::SUCCESS
            }
        },
        sub => exec_sub(sub, &args[1..]),
    }
}

fn exec_sub(name: &str, rest: &[String]) -> ExitCode {
    if !valid_subcommand_name(name) {
        eprintln!("humos: invalid subcommand {name:?}");
        return ExitCode::FAILURE;
    }
    let bin = format!("humos-{name}");
    match Command::new(&bin).args(rest).status() {
        Ok(status) => match status.code() {
            Some(0) => ExitCode::SUCCESS,
            _ => ExitCode::FAILURE,
        },
        Err(_) => {
            eprintln!("humos: no such subcommand: {name} (looked for `{bin}` in $PATH)");
            print_discovered();
            ExitCode::FAILURE
        }
    }
}

fn valid_subcommand_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn list_subcommands() -> ExitCode {
    print!("{USAGE}");
    print_discovered();
    ExitCode::SUCCESS
}

fn print_discovered() {
    let found = discover_subcommands();
    if found.is_empty() {
        println!("\nNo humos-* binaries found in $PATH.");
    } else {
        println!("\nDiscovered subcommands:");
        for name in found {
            println!("  {name}");
        }
    }
}

fn discover_subcommands() -> Vec<String> {
    let path = match env::var_os("PATH") {
        Some(p) => p,
        None => return Vec::new(),
    };
    let self_path: Option<PathBuf> = env::current_exe().ok();
    let mut out: BTreeSet<String> = BTreeSet::new();
    for dir in env::split_paths(&path) {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if !ft.is_file() && !ft.is_symlink() {
                continue;
            }
            if let Some(self_p) = &self_path {
                if entry.path() == *self_p {
                    continue;
                }
            }
            let Some(name) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if let Some(rest) = name.strip_prefix("humos-") {
                if !rest.is_empty() && !rest.contains('.') {
                    out.insert(rest.to_string());
                }
            }
        }
    }
    out.into_iter().collect()
}
