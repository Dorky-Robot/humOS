//! File-based prompt collections.
//!
//! Prompts live at `~/.humOS/prompts/<name>/template` and act like Excel
//! functions: pick a prompt, apply it to an email, get a result stored
//! per-message at `~/.humOS/mail/<account>/.<prompt-name>/<filename>`.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// A discovered prompt definition.
#[derive(Debug, Clone)]
pub struct PromptDef {
    pub name: String,
    pub description: String,
    pub template: String,
}

/// Discover all prompts under the prompts directory.
pub fn discover_prompts(prompts_dir: &Path) -> Vec<PromptDef> {
    let mut prompts = Vec::new();
    let entries = match fs::read_dir(prompts_dir) {
        Ok(e) => e,
        Err(_) => return prompts,
    };

    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = match entry.file_name().to_str() {
            Some(n) => n.to_string(),
            None => continue,
        };
        if name.starts_with('.') {
            continue;
        }
        if let Ok(def) = read_prompt(prompts_dir, &name) {
            prompts.push(def);
        }
    }

    prompts.sort_by(|a, b| a.name.cmp(&b.name));
    prompts
}

/// Read a single prompt by name.
pub fn read_prompt(prompts_dir: &Path, name: &str) -> Result<PromptDef> {
    let dir = prompts_dir.join(name);
    let template = fs::read_to_string(dir.join("template"))
        .with_context(|| format!("reading {}/template", dir.display()))?;
    let description = fs::read_to_string(dir.join("description"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    Ok(PromptDef {
        name: name.to_string(),
        description,
        template,
    })
}

/// Render a prompt template by substituting `{{key}}` placeholders.
pub fn render_template(template: &str, vars: &HashMap<&str, &str>) -> String {
    let mut out = template.to_string();
    for (key, value) in vars {
        out = out.replace(&format!("{{{{{key}}}}}"), value);
    }
    out
}

/// Result storage directory for a prompt applied to an account.
fn result_dir(account_dir: &Path, prompt_name: &str) -> PathBuf {
    account_dir.join(format!(".{prompt_name}"))
}

/// Read the stored result of applying a prompt to a message.
pub fn read_result(account_dir: &Path, prompt_name: &str, filename: &str) -> Option<String> {
    let path = result_dir(account_dir, prompt_name).join(filename);
    fs::read_to_string(path).ok()
}

/// Write the result of applying a prompt to a message.
pub fn write_result(
    account_dir: &Path,
    prompt_name: &str,
    filename: &str,
    result: &str,
) -> Result<()> {
    let dir = result_dir(account_dir, prompt_name);
    fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join(filename);
    fs::write(&path, result)
        .with_context(|| format!("writing {}", path.display()))
}

/// Seed a prompt if it doesn't already exist.
pub fn seed_prompt(prompts_dir: &Path, name: &str, description: &str, template: &str) -> Result<()> {
    let dir = prompts_dir.join(name);
    if dir.join("template").exists() {
        return Ok(());
    }
    fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    fs::write(dir.join("template"), template)?;
    fs::write(dir.join("description"), format!("{description}\n"))?;
    Ok(())
}

// ================= tests =================
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_template_substitutes_vars() {
        let tpl = "From: {{from}}, Subject: {{subject}}";
        let vars = HashMap::from([("from", "alice@x.com"), ("subject", "Hi")]);
        assert_eq!(render_template(tpl, &vars), "From: alice@x.com, Subject: Hi");
    }

    #[test]
    fn render_template_leaves_unknown_placeholders() {
        let tpl = "{{known}} and {{unknown}}";
        let vars = HashMap::from([("known", "yes")]);
        assert_eq!(render_template(tpl, &vars), "yes and {{unknown}}");
    }

    #[test]
    fn write_and_read_result_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        write_result(tmp.path(), "summarize", "msg1", "This is a summary.").unwrap();
        assert_eq!(
            read_result(tmp.path(), "summarize", "msg1"),
            Some("This is a summary.".to_string())
        );
    }

    #[test]
    fn read_result_missing_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(read_result(tmp.path(), "summarize", "nope"), None);
    }

    #[test]
    fn seed_prompt_creates_files() {
        let tmp = tempfile::tempdir().unwrap();
        seed_prompt(tmp.path(), "test", "A test prompt", "Hello {{name}}").unwrap();
        let def = read_prompt(tmp.path(), "test").unwrap();
        assert_eq!(def.name, "test");
        assert_eq!(def.description, "A test prompt");
        assert_eq!(def.template, "Hello {{name}}");
    }

    #[test]
    fn seed_prompt_does_not_overwrite_existing() {
        let tmp = tempfile::tempdir().unwrap();
        seed_prompt(tmp.path(), "test", "v1", "original").unwrap();
        seed_prompt(tmp.path(), "test", "v2", "overwritten").unwrap();
        let def = read_prompt(tmp.path(), "test").unwrap();
        assert_eq!(def.template, "original");
    }

    #[test]
    fn discover_prompts_lists_dirs_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        seed_prompt(tmp.path(), "zeta", "Z", "z").unwrap();
        seed_prompt(tmp.path(), "alpha", "A", "a").unwrap();
        let prompts = discover_prompts(tmp.path());
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[0].name, "alpha");
        assert_eq!(prompts[1].name, "zeta");
    }

    #[test]
    fn discover_prompts_skips_dotdirs() {
        let tmp = tempfile::tempdir().unwrap();
        seed_prompt(tmp.path(), ".hidden", "H", "h").unwrap();
        seed_prompt(tmp.path(), "visible", "V", "v").unwrap();
        let prompts = discover_prompts(tmp.path());
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].name, "visible");
    }

    #[test]
    fn discover_prompts_empty_dir_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(discover_prompts(tmp.path()).is_empty());
    }

    #[test]
    fn discover_prompts_missing_dir_returns_empty() {
        assert!(discover_prompts(Path::new("/nonexistent")).is_empty());
    }

    #[test]
    fn read_prompt_missing_description_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bare");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("template"), "hello").unwrap();
        let def = read_prompt(tmp.path(), "bare").unwrap();
        assert_eq!(def.description, "");
        assert_eq!(def.template, "hello");
    }
}
