//! AI-powered mail triage via a local Ollama model.
//!
//! Sends batches of email headers (From + Subject) to the Ollama API and gets
//! back a classification for each: `important`, `actionable`, `newsletter`,
//! or `spam`. Results are stored per-message as small files under
//! `~/.humOS/mail/<acct>/.triage/<filename>`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const OLLAMA_URL: &str = "http://localhost:11434/api/generate";
const DEFAULT_MODEL: &str = "gemma4:latest";
const BATCH_SIZE: usize = 20;

// ---- classification ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Label {
    Important,
    Actionable,
    Newsletter,
    Spam,
    Unknown,
}

impl Label {
    pub fn as_str(&self) -> &'static str {
        match self {
            Label::Important => "important",
            Label::Actionable => "actionable",
            Label::Newsletter => "newsletter",
            Label::Spam => "spam",
            Label::Unknown => "unknown",
        }
    }

    pub fn css_class(&self) -> &'static str {
        match self {
            Label::Important => "label-important",
            Label::Actionable => "label-actionable",
            Label::Newsletter => "label-newsletter",
            Label::Spam => "label-spam",
            Label::Unknown => "label-unknown",
        }
    }
}

pub fn parse_label(s: &str) -> Label {
    match s.trim().to_ascii_lowercase().as_str() {
        "important" => Label::Important,
        "actionable" => Label::Actionable,
        "newsletter" => Label::Newsletter,
        "spam" => Label::Spam,
        _ => Label::Unknown,
    }
}

// ---- triage storage ----

fn triage_dir(account_dir: &Path) -> PathBuf {
    account_dir.join(".triage")
}

/// Read the triage label for a message, if it exists.
pub fn read_label(account_dir: &Path, filename: &str) -> Option<Label> {
    let path = triage_dir(account_dir).join(filename);
    let raw = fs::read_to_string(path).ok()?;
    Some(parse_label(&raw))
}

/// Write a triage label for a message.
pub fn write_label(account_dir: &Path, filename: &str, label: &Label) -> Result<()> {
    let dir = triage_dir(account_dir);
    fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(filename);
    fs::write(&path, format!("{}\n", label.as_str()))
        .with_context(|| format!("writing {}", path.display()))
}

/// List filenames that already have triage labels.
pub fn triaged_filenames(account_dir: &Path) -> std::collections::HashSet<String> {
    let dir = triage_dir(account_dir);
    let mut set = std::collections::HashSet::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                set.insert(name.to_string());
            }
        }
    }
    set
}

// ---- prompt building ----

/// One email header pair for classification.
#[derive(Debug, Clone)]
pub struct HeaderPair {
    pub filename: String,
    pub from: String,
    pub subject: String,
}

/// Build the classification prompt for a batch of headers.
pub fn build_prompt(batch: &[HeaderPair]) -> String {
    let mut prompt = String::from(
        "Classify each email as exactly one of: important, actionable, newsletter, spam.\n\
         Rules:\n\
         - important: personal mail, work mail, anything needing human attention\n\
         - actionable: bills, receipts, password resets, verification codes, shipping notifications\n\
         - newsletter: marketing, digests, promotions, social media notifications\n\
         - spam: unsolicited, scams, phishing\n\n\
         Reply with ONLY the classification for each email, one per line, in order. \
         No numbering, no explanation.\n\n",
    );
    for (i, h) in batch.iter().enumerate() {
        prompt.push_str(&format!(
            "{}. From: {}, Subject: {}\n",
            i + 1,
            h.from,
            h.subject
        ));
    }
    prompt
}

/// Parse the model response into labels, one per line.
pub fn parse_response(response: &str, expected: usize) -> Vec<Label> {
    let mut labels: Vec<Label> = response
        .lines()
        .map(|line| {
            // Strip leading numbering like "1. " or "1) "
            let cleaned = line
                .trim()
                .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == ')' || c == ' ');
            parse_label(cleaned)
        })
        .filter(|l| *l != Label::Unknown || !response.trim().is_empty())
        .collect();

    // If model returned fewer lines than expected, pad with Unknown.
    labels.resize(expected, Label::Unknown);
    labels.truncate(expected);
    labels
}

/// Triage a single message by filename.  Reads the raw email from
/// INBOX/new, classifies it using headers + body, and writes the label.
pub fn triage_one_message(account_dir: &Path, filename: &str, model: Option<&str>) -> Result<Label> {
    let model = model.unwrap_or(DEFAULT_MODEL);
    let path = account_dir.join("INBOX").join("new").join(filename);
    let raw = fs::read(&path)
        .with_context(|| format!("reading {}", path.display()))?;
    let parser = mail_parser::MessageParser::default();
    let msg = parser.parse(&raw)
        .ok_or_else(|| anyhow::anyhow!("could not parse {filename}"))?;

    let from = extract_from(&msg);
    let subject = msg.subject().unwrap_or("(no subject)").to_string();
    let body = msg.body_text(0).unwrap_or_default();

    let prompt = format!(
        "Classify this email as exactly one of: important, actionable, newsletter, spam.\n\
         Rules:\n\
         - important: personal mail, work mail, anything needing human attention\n\
         - actionable: bills, receipts, password resets, verification codes, shipping notifications\n\
         - newsletter: marketing, digests, promotions, social media notifications\n\
         - spam: unsolicited, scams, phishing\n\n\
         Reply with ONLY the classification. No explanation.\n\n\
         From: {from}\nSubject: {subject}\n\n{body}"
    );
    let response = ollama_generate(model, &prompt)
        .context("ollama generate")?;
    let labels = parse_response(&response, 1);
    let label = labels.into_iter().next().unwrap_or(Label::Unknown);
    write_label(account_dir, filename, &label)?;
    Ok(label)
}

// ---- Ollama API ----

#[derive(Serialize)]
struct OllamaRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
}

#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
}

/// Send a prompt to Ollama and return the response text.
pub fn ollama_generate(model: &str, prompt: &str) -> Result<String> {
    let body = serde_json::to_string(&OllamaRequest {
        model,
        prompt,
        stream: false,
    })?;

    let resp = ureq::post(OLLAMA_URL)
        .set("Content-Type", "application/json")
        .send_string(&body)
        .context("calling Ollama API")?;

    let parsed: OllamaResponse = resp.into_json().context("parsing Ollama response")?;
    Ok(parsed.response)
}

// ---- batch triage orchestration ----

/// Collect un-triaged message headers from an account's INBOX/new.
pub fn pending_headers(account_dir: &Path) -> Vec<HeaderPair> {
    let new_dir = account_dir.join("INBOX").join("new");
    if !new_dir.exists() {
        return Vec::new();
    }

    let already = triaged_filenames(account_dir);
    let mut pending: Vec<HeaderPair> = Vec::new();

    let entries = match fs::read_dir(&new_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let filename = match entry.file_name().to_str() {
            Some(n) => n.to_string(),
            None => continue,
        };
        if already.contains(&filename) {
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
        pending.push(HeaderPair {
            filename,
            from,
            subject,
        });
    }

    pending
}

/// Triage result from a single batch.
pub struct TriageBatchResult {
    pub triaged: usize,
    pub remaining: usize,
}

/// Triage up to `limit` un-triaged messages.  Returns how many were
/// classified and how many are still pending.
pub fn triage_batch(account_dir: &Path, model: Option<&str>, limit: usize) -> Result<TriageBatchResult> {
    let model = model.unwrap_or(DEFAULT_MODEL);
    let pending = pending_headers(account_dir);

    if pending.is_empty() {
        return Ok(TriageBatchResult { triaged: 0, remaining: 0 });
    }

    let batch = &pending[..pending.len().min(limit)];
    let prompt = build_prompt(batch);
    let response = ollama_generate(model, &prompt)
        .context("ollama generate")?;
    let labels = parse_response(&response, batch.len());

    for (header, label) in batch.iter().zip(labels.iter()) {
        write_label(account_dir, &header.filename, label)?;
    }

    Ok(TriageBatchResult {
        triaged: batch.len(),
        remaining: pending.len() - batch.len(),
    })
}

/// Triage all un-triaged messages in an account's INBOX/new.
/// Returns the number of messages newly triaged.
pub fn triage_account(account_dir: &Path, model: Option<&str>) -> Result<usize> {
    let model = model.unwrap_or(DEFAULT_MODEL);
    let pending = pending_headers(account_dir);

    if pending.is_empty() {
        return Ok(0);
    }

    let mut count = 0;
    for batch in pending.chunks(BATCH_SIZE) {
        let prompt = build_prompt(batch);
        let response = ollama_generate(model, &prompt)
            .context("ollama generate")?;
        let labels = parse_response(&response, batch.len());

        for (header, label) in batch.iter().zip(labels.iter()) {
            write_label(account_dir, &header.filename, label)?;
            count += 1;
        }

        eprint!(".");
    }

    Ok(count)
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

// ================= tests =================
#[cfg(test)]
mod tests {
    use super::*;

    // ---- parse_label ----
    #[test]
    fn parse_label_known_values() {
        assert_eq!(parse_label("important"), Label::Important);
        assert_eq!(parse_label("actionable"), Label::Actionable);
        assert_eq!(parse_label("newsletter"), Label::Newsletter);
        assert_eq!(parse_label("spam"), Label::Spam);
    }

    #[test]
    fn parse_label_case_insensitive() {
        assert_eq!(parse_label("IMPORTANT"), Label::Important);
        assert_eq!(parse_label("Newsletter"), Label::Newsletter);
    }

    #[test]
    fn parse_label_trims_whitespace() {
        assert_eq!(parse_label("  spam  \n"), Label::Spam);
    }

    #[test]
    fn parse_label_unknown_garbage() {
        assert_eq!(parse_label("banana"), Label::Unknown);
    }

    // ---- build_prompt ----
    #[test]
    fn build_prompt_includes_all_headers() {
        let batch = vec![
            HeaderPair {
                filename: "a".into(),
                from: "alice@x.com".into(),
                subject: "Hi".into(),
            },
            HeaderPair {
                filename: "b".into(),
                from: "bob@x.com".into(),
                subject: "Bye".into(),
            },
        ];
        let prompt = build_prompt(&batch);
        assert!(prompt.contains("1. From: alice@x.com, Subject: Hi"));
        assert!(prompt.contains("2. From: bob@x.com, Subject: Bye"));
        assert!(prompt.contains("important"));
        assert!(prompt.contains("actionable"));
        assert!(prompt.contains("newsletter"));
        assert!(prompt.contains("spam"));
    }

    // ---- parse_response ----
    #[test]
    fn parse_response_clean_lines() {
        let resp = "important\nnewsletter\nspam\n";
        let labels = parse_response(resp, 3);
        assert_eq!(labels, vec![Label::Important, Label::Newsletter, Label::Spam]);
    }

    #[test]
    fn parse_response_with_numbering() {
        let resp = "1. important\n2. newsletter\n3. spam\n";
        let labels = parse_response(resp, 3);
        assert_eq!(labels, vec![Label::Important, Label::Newsletter, Label::Spam]);
    }

    #[test]
    fn parse_response_pads_if_short() {
        let resp = "important\n";
        let labels = parse_response(resp, 3);
        assert_eq!(labels.len(), 3);
        assert_eq!(labels[0], Label::Important);
        assert_eq!(labels[1], Label::Unknown);
    }

    #[test]
    fn parse_response_truncates_if_long() {
        let resp = "important\nspam\nnewsletter\nactionable\n";
        let labels = parse_response(resp, 2);
        assert_eq!(labels.len(), 2);
    }

    // ---- triage storage ----
    #[test]
    fn write_and_read_label_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        write_label(tmp.path(), "msg1", &Label::Important).unwrap();
        assert_eq!(read_label(tmp.path(), "msg1"), Some(Label::Important));
    }

    #[test]
    fn read_label_missing_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(read_label(tmp.path(), "nope"), None);
    }

    #[test]
    fn triaged_filenames_lists_existing() {
        let tmp = tempfile::tempdir().unwrap();
        write_label(tmp.path(), "a", &Label::Spam).unwrap();
        write_label(tmp.path(), "b", &Label::Newsletter).unwrap();
        let set = triaged_filenames(tmp.path());
        assert!(set.contains("a"));
        assert!(set.contains("b"));
        assert!(!set.contains("c"));
    }

    #[test]
    fn triaged_filenames_empty_dir_returns_empty_set() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(triaged_filenames(tmp.path()).is_empty());
    }
}
