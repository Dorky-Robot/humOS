//! Shared CLI conventions for every `humos-*` binary.
//!
//! The surface is designed for **agents**, not humans: `--json` emits a
//! stable envelope, `--help` prints usage, and errors go to stderr (or the
//! envelope) with a non-zero exit. Binaries stay thin — they parse their
//! own positional args out of [`Flags::rest`] and return a serializable
//! `data` plus a human-text rendering; this module handles the rest.

use serde::Serialize;
use std::process::ExitCode;

/// Parsed shared flags plus anything we didn't recognize (positional args,
/// sub-flags the binary knows about).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Flags {
    pub json: bool,
    pub help: bool,
    pub rest: Vec<String>,
}

impl Flags {
    /// Extract shared flags (`--json`, `--help`/`-h`) from an arg iterator.
    /// Unknown args are collected into [`Flags::rest`] preserving order.
    pub fn parse<I>(args: I) -> Self
    where
        I: IntoIterator<Item = String>,
    {
        let mut json = false;
        let mut help = false;
        let mut rest = Vec::new();
        for arg in args {
            match arg.as_str() {
                "--json" => json = true,
                "--help" | "-h" => help = true,
                _ => rest.push(arg),
            }
        }
        Flags { json, help, rest }
    }
}

/// What a binary's work closure returns.
pub struct Output<T: Serialize> {
    pub data: T,
    pub human: String,
}

#[derive(Serialize)]
struct Envelope<'a, T: Serialize> {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<&'a T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

/// Render a success envelope as a JSON string.
pub fn envelope_ok<T: Serialize>(data: &T) -> String {
    let env = Envelope {
        ok: true,
        data: Some(data),
        error: None,
    };
    serde_json::to_string(&env).expect("envelope serializes")
}

/// Render an error envelope as a JSON string.
pub fn envelope_err(msg: &str) -> String {
    let env: Envelope<'_, ()> = Envelope {
        ok: false,
        data: None,
        error: Some(msg),
    };
    serde_json::to_string(&env).expect("envelope serializes")
}

/// Run a binary's work. Handles `--help`, `--json`, exit codes, error
/// formatting. The binary just supplies usage text and a closure returning
/// `Output { data, human }`.
pub fn run<T, F>(args: Vec<String>, usage: &str, work: F) -> ExitCode
where
    T: Serialize,
    F: FnOnce(&Flags) -> anyhow::Result<Output<T>>,
{
    let flags = Flags::parse(args);
    if flags.help {
        println!("{usage}");
        return ExitCode::SUCCESS;
    }
    match work(&flags) {
        Ok(Output { data, human }) => {
            if flags.json {
                println!("{}", envelope_ok(&data));
            } else {
                print!("{human}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            let msg = format!("{e:#}");
            if flags.json {
                println!("{}", envelope_err(&msg));
            } else {
                eprintln!("error: {msg}");
            }
            ExitCode::FAILURE
        }
    }
}

// ================= tests =================
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn parse(args: &[&str]) -> Flags {
        Flags::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn flags_parse_empty_is_all_false_and_empty_rest() {
        let f = parse(&[]);
        assert!(!f.json);
        assert!(!f.help);
        assert!(f.rest.is_empty());
    }

    #[test]
    fn flags_parse_recognizes_json() {
        assert!(parse(&["--json"]).json);
    }

    #[test]
    fn flags_parse_recognizes_help_long_and_short() {
        assert!(parse(&["--help"]).help);
        assert!(parse(&["-h"]).help);
    }

    #[test]
    fn flags_parse_collects_unknown_args_as_rest_preserving_order() {
        let f = parse(&["gmail", "--model", "llama3", "--json"]);
        assert!(f.json);
        assert_eq!(f.rest, vec!["gmail", "--model", "llama3"]);
    }

    #[test]
    fn envelope_ok_serializes_ok_true_with_data_and_no_error() {
        let s = envelope_ok(&serde_json::json!({"count": 2}));
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["data"]["count"], 2);
        assert!(v.get("error").is_none());
    }

    #[test]
    fn envelope_err_serializes_ok_false_with_error_and_no_data() {
        let s = envelope_err("boom");
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"], "boom");
        assert!(v.get("data").is_none());
    }
}
