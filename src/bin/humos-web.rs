//! humos-web — minimal localhost browser view for humOS.
//!
//! Binds `127.0.0.1:8765` and serves a single HTML page that lists every mail
//! account discovered under `~/.humOS/mail/` along with its unread messages.
//! Each message has an "Archive" button (a tiny POST form) that moves it into
//! `Archive/cur/`. There is no JavaScript, no CSS framework, no auth, and no
//! TLS — this is a localhost UI for one human, not a server.
//!
//! HTTP/1.1 is hand-rolled over `std::net::TcpListener` so the binary stays
//! self-contained (no extra deps). All the interesting logic lives in pure
//! functions (`render_index_html`, `route`, `parse_request`, `parse_form`)
//! that are exercised by the test module at the bottom of this file.

use anyhow::{Context, Result};
use humos::account::{
    auto_provider_config, create_account, is_fetchable, read_imap_config,
};
use humos::imap_client::fetch_inbox;
use humos::mail::{
    archive_message, discover_accounts, mail_report, AccountReport,
};
use humos::prompts;
use humos::triage;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

const BIND_ADDR: &str = "127.0.0.1:8765";

// ---- request / response model ----

#[derive(Debug, PartialEq, Eq)]
struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

#[derive(Debug, PartialEq, Eq)]
struct Response {
    status: u16,
    content_type: &'static str,
    extra_headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Response {
    fn html(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Response {
            status,
            content_type: "text/html; charset=utf-8",
            extra_headers: Vec::new(),
            body: body.into(),
        }
    }
    fn text(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Response {
            status,
            content_type: "text/plain; charset=utf-8",
            extra_headers: Vec::new(),
            body: body.into(),
        }
    }
    fn redirect(location: &str) -> Self {
        Response {
            status: 303,
            content_type: "text/plain; charset=utf-8",
            extra_headers: vec![("Location".to_string(), location.to_string())],
            body: b"See Other".to_vec(),
        }
    }
}

// ---- request parsing ----

/// Parse an HTTP/1.1 request from a buffered reader. Returns `Ok(None)` on EOF
/// (closed connection with no bytes), `Err` on a malformed request.
fn parse_request<R: BufRead>(reader: &mut R) -> Result<Option<Request>> {
    let mut request_line = String::new();
    let n = reader
        .read_line(&mut request_line)
        .context("reading request line")?;
    if n == 0 {
        return Ok(None);
    }
    let trimmed = request_line.trim_end_matches(['\r', '\n']);
    let mut parts = trimmed.splitn(3, ' ');
    let method = parts.next().context("missing method")?.to_string();
    let path = parts.next().context("missing path")?.to_string();
    let _version = parts.next().context("missing version")?;

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).context("reading header")?;
        if n == 0 {
            break;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader.read_exact(&mut body).context("reading body")?;
    }

    Ok(Some(Request {
        method,
        path,
        body,
    }))
}

// ---- url + form helpers ----

fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(h), Some(l)) = (hi, lo) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_form(body: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for pair in body.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        out.insert(url_decode(k), url_decode(v));
    }
    out
}

// ---- rendering ----

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

const CSS: &str = "\
body{font:14px/1.4 -apple-system,system-ui,sans-serif;max-width:780px;\
margin:2rem auto;padding:0 1rem;color:#222}\
h1{font-size:1.4rem;margin:0 0 1rem}\
h2{font-size:1.05rem;margin:1.5rem 0 .25rem;color:#444}\
ul{list-style:none;padding:0;margin:0}\
li{display:flex;gap:.5rem;align-items:baseline;padding:.35rem 0;\
border-bottom:1px solid #eee}\
.from{color:#666;flex:0 0 11rem;overflow:hidden;\
text-overflow:ellipsis;white-space:nowrap}\
.subj{flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}\
button{font:inherit;cursor:pointer;border:1px solid #bbb;\
background:#f6f6f6;border-radius:4px;padding:.15rem .55rem}\
button:hover{background:#eaeaea}\
.empty{color:#777;font-style:italic}\
nav{margin:0 0 1rem;display:flex;gap:1rem;align-items:center}\
nav a{color:#0066cc;text-decoration:none}\
nav a:hover{text-decoration:underline}\
.flash{padding:.5rem;margin:0 0 1rem;border-radius:4px}\
.flash-ok{background:#e6f9e6;border:1px solid #6c6}\
.flash-err{background:#fce6e6;border:1px solid #c66}\
.label{font-size:.75rem;padding:.1rem .4rem;border-radius:3px;white-space:nowrap}\
.label-important{background:#fde68a;color:#92400e}\
.label-actionable{background:#bfdbfe;color:#1e40af}\
.label-newsletter{background:#e0e7ff;color:#4338ca}\
.label-spam{background:#fecaca;color:#991b1b}\
.label-unknown{background:#f3f4f6;color:#6b7280}\
label{display:block;margin:.5rem 0 .2rem;font-weight:600}\
input[type=text],input[type=email],input[type=password]\
{font:inherit;padding:.3rem .5rem;width:100%;max-width:24rem;\
border:1px solid #bbb;border-radius:4px}\
fieldset{border:1px solid #ddd;border-radius:6px;padding:1rem;margin:0 0 1rem}\
legend{font-weight:600;padding:0 .3rem}\
.hint{font-size:.85rem;color:#666;margin:.3rem 0 .8rem}\
.msg-meta{margin:0 0 1rem;color:#555}\
.msg-meta dt{font-weight:600;float:left;width:5rem;clear:left}\
.msg-meta dd{margin:0 0 .3rem}\
.msg-body{white-space:pre-wrap;font-family:inherit;background:#fafafa;\
border:1px solid #eee;border-radius:4px;padding:1rem;margin:1rem 0;\
max-height:60vh;overflow:auto}\
.prompt-results{margin:1rem 0}\
.prompt-results h3{font-size:.95rem;margin:.8rem 0 .3rem}\
.prompt-results pre{background:#f5f5f5;border:1px solid #e0e0e0;\
border-radius:4px;padding:.75rem;white-space:pre-wrap;font-size:.85rem}\
.actions{display:flex;gap:.5rem;flex-wrap:wrap;margin:1rem 0}";

fn page_head(title: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <title>{title}</title><style>{CSS}</style></head><body>"
    )
}

fn render_index_html(
    reports: &[AccountReport],
    mail_root: &Path,
    flash: Option<&str>,
) -> String {
    let mut s = page_head("humOS");
    s.push_str("<h1>humOS</h1>");
    s.push_str("<nav>");
    s.push_str("<a href=\"/add-account\">+ Add account</a>");
    s.push_str("<a href=\"/edit-prompt?name=triage\">Prompts</a>");
    s.push_str("<form method=\"post\" action=\"/sync\" style=\"display:inline\">");
    s.push_str("<button type=\"submit\">Sync all</button>");
    s.push_str("</form>");
    s.push_str("<form method=\"post\" action=\"/triage\" style=\"display:inline\">");
    s.push_str("<button type=\"submit\">Triage</button>");
    s.push_str("</form>");
    s.push_str("</nav>");

    if let Some(msg) = flash {
        let cls = if msg.starts_with("error") {
            "flash flash-err"
        } else {
            "flash flash-ok"
        };
        s.push_str(&format!(
            "<div class=\"{cls}\">{}</div>",
            html_escape(msg)
        ));
    }

    if reports.is_empty() {
        s.push_str(
            "<p class=\"empty\">No mail accounts yet. \
             <a href=\"/add-account\">Add one</a> to get started.</p>",
        );
    } else {
        for r in reports {
            s.push_str(&format!(
                "<h2>{} <small>({} unread, {} read)</small></h2>",
                html_escape(&r.label),
                r.unread_count,
                r.read_count
            ));
            if r.recent.is_empty() {
                s.push_str("<p class=\"empty\">nothing new</p>");
                continue;
            }
            let account_dir = mail_root.join(&r.account);
            s.push_str("<ul>");
            for m in &r.recent {
                s.push_str("<li>");
                // Show triage label if available.
                if let Some(label) = triage::read_label(&account_dir, &m.filename) {
                    s.push_str(&format!(
                        "<span class=\"label {}\">{}</span>",
                        label.css_class(),
                        label.as_str()
                    ));
                }
                s.push_str(&format!(
                    "<span class=\"from\">{}</span>",
                    html_escape(&m.from)
                ));
                s.push_str(&format!(
                    "<a class=\"subj\" href=\"/message?account={}&filename={}\">{}</a>",
                    url_encode(&r.account),
                    url_encode(&m.filename),
                    html_escape(&m.subject)
                ));
                // Triage button (only if not already triaged).
                if triage::read_label(&account_dir, &m.filename).is_none() {
                    s.push_str("<form method=\"post\" action=\"/triage-one\" style=\"display:inline\">");
                    s.push_str(&format!(
                        "<input type=\"hidden\" name=\"account\" value=\"{}\">",
                        html_escape(&r.account)
                    ));
                    s.push_str(&format!(
                        "<input type=\"hidden\" name=\"filename\" value=\"{}\">",
                        html_escape(&m.filename)
                    ));
                    s.push_str("<button type=\"submit\">Triage</button>");
                    s.push_str("</form>");
                }
                s.push_str("<form method=\"post\" action=\"/archive\" style=\"display:inline\">");
                s.push_str(&format!(
                    "<input type=\"hidden\" name=\"account\" value=\"{}\">",
                    html_escape(&r.account)
                ));
                s.push_str(&format!(
                    "<input type=\"hidden\" name=\"filename\" value=\"{}\">",
                    html_escape(&m.filename)
                ));
                s.push_str("<button type=\"submit\">Archive</button>");
                s.push_str("</form>");
                s.push_str("</li>");
            }
            s.push_str("</ul>");
        }
    }
    s.push_str("</body></html>");
    s
}

fn render_add_account_html(error: Option<&str>) -> String {
    let mut s = page_head("humOS - Add account");
    s.push_str("<h1><a href=\"/\" style=\"text-decoration:none;color:inherit\">humOS</a></h1>");
    s.push_str("<h2>Add mail account</h2>");

    if let Some(msg) = error {
        s.push_str(&format!(
            "<div class=\"flash flash-err\">{}</div>",
            html_escape(msg)
        ));
    }

    s.push_str("<form method=\"post\" action=\"/add-account\">");
    s.push_str("<fieldset><legend>Account</legend>");

    s.push_str("<label for=\"name\">Account name</label>");
    s.push_str("<input type=\"text\" id=\"name\" name=\"name\" \
                placeholder=\"gmail\" required>");
    s.push_str("<p class=\"hint\">A short label (letters, numbers, hyphens). \
                Used as the folder name under ~/.humOS/mail/.</p>");

    s.push_str("<label for=\"email\">Email address</label>");
    s.push_str("<input type=\"email\" id=\"email\" name=\"email\" \
                placeholder=\"you@gmail.com\" required>");

    s.push_str("<label for=\"password\">App password</label>");
    s.push_str("<input type=\"password\" id=\"password\" name=\"password\" required>");
    s.push_str("<p class=\"hint\">\
                Not your regular password. Generate an app-specific password:<br>\
                Gmail: <a href=\"https://myaccount.google.com/apppasswords\" \
                target=\"_blank\">myaccount.google.com/apppasswords</a><br>\
                iCloud: <a href=\"https://appleid.apple.com/account/manage/security\" \
                target=\"_blank\">appleid.apple.com</a> &rarr; \
                Sign-In &amp; Security &rarr; App-Specific Passwords\
                </p>");

    s.push_str("</fieldset>");
    s.push_str("<button type=\"submit\" style=\"margin-top:.5rem;padding:.4rem 1.2rem\">\
                Add account</button>");
    s.push_str("</form>");
    s.push_str("</body></html>");
    s
}

fn render_edit_prompt_html(
    prompts_dir: &Path,
    name: &str,
    flash: Option<&str>,
) -> String {
    let mut s = page_head(&format!("humOS - Edit: {name}"));
    s.push_str("<h1><a href=\"/\" style=\"text-decoration:none;color:inherit\">humOS</a></h1>");
    s.push_str(&format!("<h2>Edit prompt: {}</h2>", html_escape(name)));

    if let Some(msg) = flash {
        let cls = if msg.starts_with("error") {
            "flash flash-err"
        } else {
            "flash flash-ok"
        };
        s.push_str(&format!(
            "<div class=\"{cls}\">{}</div>",
            html_escape(msg)
        ));
    }

    let def = prompts::read_prompt(prompts_dir, name);
    let (template, description) = match &def {
        Ok(d) => (d.template.as_str(), d.description.as_str()),
        Err(_) => ("", ""),
    };

    s.push_str("<form method=\"post\" action=\"/edit-prompt\">");
    s.push_str(&format!(
        "<input type=\"hidden\" name=\"name\" value=\"{}\">",
        html_escape(name)
    ));

    s.push_str("<label for=\"description\">Description</label>");
    s.push_str(&format!(
        "<input type=\"text\" id=\"description\" name=\"description\" \
         value=\"{}\" style=\"margin-bottom:.5rem\">",
        html_escape(description)
    ));

    s.push_str("<label for=\"template\">Template</label>");
    s.push_str("<p class=\"hint\">Available variables: \
                <code>{{{{from}}}}</code>, <code>{{{{subject}}}}</code>, \
                <code>{{{{body}}}}</code></p>");
    s.push_str(&format!(
        "<textarea id=\"template\" name=\"template\" rows=\"20\" \
         style=\"font:13px/1.4 monospace;width:100%;max-width:40rem;\
         border:1px solid #bbb;border-radius:4px;padding:.5rem\">{}</textarea>",
        html_escape(template)
    ));

    s.push_str("<br><button type=\"submit\" style=\"margin-top:.5rem;padding:.4rem 1.2rem\">\
                Save</button>");
    s.push_str("</form>");

    // List all prompts for navigation.
    let all = prompts::discover_prompts(prompts_dir);
    if all.len() > 1 {
        s.push_str("<h3 style=\"margin-top:2rem\">All prompts</h3><ul>");
        for p in &all {
            s.push_str(&format!(
                "<li><a href=\"/edit-prompt?name={}\">{}</a> — {}</li>",
                url_encode(&p.name),
                html_escape(&p.name),
                html_escape(&p.description),
            ));
        }
        s.push_str("</ul>");
    }

    s.push_str("</body></html>");
    s
}

fn render_message_html(
    account: &str,
    filename: &str,
    mail_root: &Path,
    prompts_dir: &Path,
    flash: Option<&str>,
) -> String {
    let mut s = page_head("humOS - Message");
    s.push_str("<h1><a href=\"/\" style=\"text-decoration:none;color:inherit\">humOS</a></h1>");

    if let Some(msg) = flash {
        let cls = if msg.starts_with("error") {
            "flash flash-err"
        } else {
            "flash flash-ok"
        };
        s.push_str(&format!(
            "<div class=\"{cls}\">{}</div>",
            html_escape(msg)
        ));
    }

    let account_dir = mail_root.join(account);
    let msg_path = account_dir.join("INBOX").join("new").join(filename);
    let raw = match std::fs::read(&msg_path) {
        Ok(b) => b,
        Err(_) => {
            s.push_str("<p>Message not found.</p></body></html>");
            return s;
        }
    };
    let parser = mail_parser::MessageParser::default();
    let parsed = match parser.parse(&raw) {
        Some(m) => m,
        None => {
            s.push_str("<p>Could not parse message.</p></body></html>");
            return s;
        }
    };

    let from = parsed
        .from()
        .and_then(|a| match a {
            mail_parser::Address::List(list) => list.first().map(|addr| {
                match (&addr.name, &addr.address) {
                    (Some(n), Some(a)) => format!("{n} <{a}>"),
                    (None, Some(a)) => a.to_string(),
                    (Some(n), None) => n.to_string(),
                    (None, None) => "(unknown)".into(),
                }
            }),
            _ => None,
        })
        .unwrap_or_else(|| "(unknown)".into());
    let subject = parsed.subject().unwrap_or("(no subject)");
    let date = parsed.date().map(|d| d.to_rfc3339()).unwrap_or_default();
    let body_text = parsed
        .body_text(0)
        .unwrap_or_default();

    // Header metadata.
    s.push_str("<dl class=\"msg-meta\">");
    s.push_str(&format!("<dt>From</dt><dd>{}</dd>", html_escape(&from)));
    s.push_str(&format!("<dt>Subject</dt><dd>{}</dd>", html_escape(subject)));
    if !date.is_empty() {
        s.push_str(&format!("<dt>Date</dt><dd>{}</dd>", html_escape(&date)));
    }
    // Triage label.
    if let Some(label) = triage::read_label(&account_dir, filename) {
        s.push_str(&format!(
            "<dt>Triage</dt><dd><span class=\"label {}\">{}</span></dd>",
            label.css_class(),
            label.as_str()
        ));
    }
    s.push_str("</dl>");

    // Email body.
    s.push_str(&format!(
        "<div class=\"msg-body\">{}</div>",
        html_escape(&body_text)
    ));

    // Action buttons.
    s.push_str("<div class=\"actions\">");
    // Triage (if not already done).
    if triage::read_label(&account_dir, filename).is_none() {
        s.push_str("<form method=\"post\" action=\"/triage-one\">");
        s.push_str(&format!(
            "<input type=\"hidden\" name=\"account\" value=\"{}\">",
            html_escape(account)
        ));
        s.push_str(&format!(
            "<input type=\"hidden\" name=\"filename\" value=\"{}\">",
            html_escape(filename)
        ));
        s.push_str("<button type=\"submit\">Triage</button>");
        s.push_str("</form>");
    }
    // Archive.
    s.push_str("<form method=\"post\" action=\"/archive\">");
    s.push_str(&format!(
        "<input type=\"hidden\" name=\"account\" value=\"{}\">",
        html_escape(account)
    ));
    s.push_str(&format!(
        "<input type=\"hidden\" name=\"filename\" value=\"{}\">",
        html_escape(filename)
    ));
    s.push_str("<button type=\"submit\">Archive</button>");
    s.push_str("</form>");
    // Prompt collection buttons.
    let available_prompts = prompts::discover_prompts(prompts_dir);
    for p in &available_prompts {
        s.push_str("<form method=\"post\" action=\"/apply-prompt\">");
        s.push_str(&format!(
            "<input type=\"hidden\" name=\"account\" value=\"{}\">",
            html_escape(account)
        ));
        s.push_str(&format!(
            "<input type=\"hidden\" name=\"filename\" value=\"{}\">",
            html_escape(filename)
        ));
        s.push_str(&format!(
            "<input type=\"hidden\" name=\"prompt\" value=\"{}\">",
            html_escape(&p.name)
        ));
        s.push_str(&format!(
            "<button type=\"submit\" title=\"{}\">{}</button>",
            html_escape(&p.description),
            html_escape(&p.name)
        ));
        s.push_str("</form>");
    }
    s.push_str("</div>");

    // Show existing prompt results.
    let mut has_results = false;
    for p in &available_prompts {
        if let Some(result) = prompts::read_result(&account_dir, &p.name, filename) {
            if !has_results {
                s.push_str("<div class=\"prompt-results\">");
                has_results = true;
            }
            s.push_str(&format!("<h3>{}</h3>", html_escape(&p.name)));
            s.push_str(&format!("<pre>{}</pre>", html_escape(&result)));
        }
    }
    if has_results {
        s.push_str("</div>");
    }

    s.push_str("</body></html>");
    s
}

// ---- routing ----

fn route(req: &Request, mail_root: &Path, prompts_dir: &Path) -> Response {
    let (path, query) = match req.path.split_once('?') {
        Some((p, q)) => (p, q),
        None => (req.path.as_str(), ""),
    };
    match (req.method.as_str(), path) {
        ("GET", "/") => {
            let flash = if query.is_empty() {
                None
            } else {
                let q = parse_form(query);
                q.get("flash").cloned()
            };
            match build_reports(mail_root) {
                Ok(reports) => {
                    Response::html(
                        200,
                        render_index_html(&reports, mail_root, flash.as_deref()),
                    )
                }
                Err(e) => Response::text(500, format!("error: {e}")),
            }
        }
        ("GET", "/message") => {
            let q = parse_form(query);
            let account = match q.get("account") {
                Some(a) if !a.is_empty() => a.as_str(),
                _ => return Response::text(400, "missing account"),
            };
            let filename = match q.get("filename") {
                Some(f) if !f.is_empty() => f.as_str(),
                _ => return Response::text(400, "missing filename"),
            };
            let flash = q.get("flash").map(|s| s.as_str());
            Response::html(
                200,
                render_message_html(account, filename, mail_root, prompts_dir, flash),
            )
        }
        ("GET", "/edit-prompt") => {
            let q = parse_form(query);
            let name = match q.get("name") {
                Some(n) if !n.is_empty() => n.as_str(),
                _ => return Response::text(400, "missing prompt name"),
            };
            let flash = q.get("flash").map(|s| s.as_str());
            Response::html(200, render_edit_prompt_html(prompts_dir, name, flash))
        }
        ("POST", "/edit-prompt") => {
            let body = std::str::from_utf8(&req.body).unwrap_or("");
            let form = parse_form(body);
            let name = match form.get("name") {
                Some(n) if !n.is_empty() => n,
                _ => return Response::text(400, "missing prompt name"),
            };
            let template = match form.get("template") {
                Some(t) => t,
                _ => return Response::text(400, "missing template"),
            };
            let description = form.get("description").map(|s| s.as_str()).unwrap_or("");
            let dir = prompts_dir.join(name.as_str());
            if let Err(e) = std::fs::create_dir_all(&dir) {
                let flash = url_encode(&format!("error: {e}"));
                return Response::redirect(&format!(
                    "/edit-prompt?name={}&flash={flash}",
                    url_encode(name)
                ));
            }
            if let Err(e) = std::fs::write(dir.join("template"), template.as_bytes()) {
                let flash = url_encode(&format!("error: {e}"));
                return Response::redirect(&format!(
                    "/edit-prompt?name={}&flash={flash}",
                    url_encode(name)
                ));
            }
            let _ = std::fs::write(dir.join("description"), format!("{description}\n"));
            let flash = url_encode("Saved");
            Response::redirect(&format!(
                "/edit-prompt?name={}&flash={flash}",
                url_encode(name)
            ))
        }
        ("GET", "/add-account") => Response::html(200, render_add_account_html(None)),
        ("POST", "/add-account") => handle_add_account(req, mail_root),
        ("POST", "/sync") => handle_sync(req, mail_root),
        ("POST", "/triage") => handle_triage(mail_root),
        ("POST", "/triage-one") => {
            let body = std::str::from_utf8(&req.body).unwrap_or("");
            let form = parse_form(body);
            let account = match form.get("account") {
                Some(a) if !a.is_empty() => a,
                _ => return Response::text(400, "missing account"),
            };
            let filename = match form.get("filename") {
                Some(f) if !f.is_empty() => f,
                _ => return Response::text(400, "missing filename"),
            };
            let account_dir = mail_root.join(account);
            let back = format!(
                "/message?account={}&filename={}",
                url_encode(account),
                url_encode(filename)
            );
            match triage::triage_one_message(&account_dir, filename, None) {
                Ok(label) => {
                    let flash = url_encode(&format!(
                        "Classified as: {}", label.as_str()
                    ));
                    Response::redirect(&format!("{back}&flash={flash}"))
                }
                Err(e) => {
                    let flash = url_encode(&format!("error: {e:#}"));
                    Response::redirect(&format!("{back}&flash={flash}"))
                }
            }
        }
        ("POST", "/apply-prompt") => {
            let body = std::str::from_utf8(&req.body).unwrap_or("");
            let form = parse_form(body);
            let account = match form.get("account") {
                Some(a) if !a.is_empty() => a,
                _ => return Response::text(400, "missing account"),
            };
            let filename = match form.get("filename") {
                Some(f) if !f.is_empty() => f,
                _ => return Response::text(400, "missing filename"),
            };
            let prompt_name = match form.get("prompt") {
                Some(p) if !p.is_empty() => p,
                _ => return Response::text(400, "missing prompt"),
            };
            let back = format!(
                "/message?account={}&filename={}",
                url_encode(account),
                url_encode(filename)
            );
            match apply_prompt_to_message(mail_root, prompts_dir, account, filename, prompt_name) {
                Ok(_) => {
                    let flash = url_encode(&format!("Applied: {prompt_name}"));
                    Response::redirect(&format!("{back}&flash={flash}"))
                }
                Err(e) => {
                    let flash = url_encode(&format!("error: {e:#}"));
                    Response::redirect(&format!("{back}&flash={flash}"))
                }
            }
        }
        ("POST", "/archive") => {
            let body = std::str::from_utf8(&req.body).unwrap_or("");
            let form = parse_form(body);
            let account = match form.get("account") {
                Some(a) if !a.is_empty() => a,
                _ => return Response::text(400, "missing account"),
            };
            let filename = match form.get("filename") {
                Some(f) if !f.is_empty() => f,
                _ => return Response::text(400, "missing filename"),
            };
            match archive_message(mail_root, account, filename) {
                Ok(_) => Response::redirect("/"),
                Err(e) => Response::text(400, format!("archive failed: {e}")),
            }
        }
        _ => Response::text(404, "not found"),
    }
}

fn handle_add_account(req: &Request, mail_root: &Path) -> Response {
    let body = std::str::from_utf8(&req.body).unwrap_or("");
    let form = parse_form(body);

    let name = match form.get("name") {
        Some(n) if !n.is_empty() => n.clone(),
        _ => return Response::html(400, render_add_account_html(Some("Account name is required"))),
    };
    let email = match form.get("email") {
        Some(e) if !e.is_empty() => e.clone(),
        _ => {
            return Response::html(400, render_add_account_html(Some("Email address is required")))
        }
    };
    let password = match form.get("password") {
        Some(p) if !p.is_empty() => p.clone(),
        _ => return Response::html(400, render_add_account_html(Some("App password is required"))),
    };

    let provider = match auto_provider_config(&email) {
        Some(p) => p,
        None => {
            return Response::html(
                400,
                render_add_account_html(Some(
                    "Unknown email provider. Only Gmail, iCloud, Outlook, Yahoo, \
                     and Fastmail are auto-configured for now.",
                )),
            )
        }
    };

    match create_account(mail_root, &name, &email, &password, &provider) {
        Ok(_) => {
            let flash = url_encode(&format!("Account '{}' added", name));
            Response::redirect(&format!("/?flash={flash}"))
        }
        Err(e) => Response::html(400, render_add_account_html(Some(&format!("{e:#}")))),
    }
}

fn handle_sync(_req: &Request, mail_root: &Path) -> Response {
    let accounts = discover_accounts(mail_root);
    let mut total = 0usize;
    let mut errors = Vec::new();

    for account in &accounts {
        let account_dir = mail_root.join(&account.name);
        if !is_fetchable(&account_dir) {
            continue;
        }
        let config = match read_imap_config(&account_dir, &account.name) {
            Ok(c) => c,
            Err(e) => {
                errors.push(format!("{}: {e:#}", account.name));
                continue;
            }
        };
        match fetch_inbox(&config, &account_dir) {
            Ok(n) => total += n,
            Err(e) => errors.push(format!("{}: {e:#}", account.name)),
        }
    }

    let flash = if errors.is_empty() {
        url_encode(&format!("Synced: {total} new message(s)"))
    } else {
        url_encode(&format!(
            "error: {} (fetched {total} new otherwise)",
            errors.join("; ")
        ))
    };
    Response::redirect(&format!("/?flash={flash}"))
}

fn handle_triage(mail_root: &Path) -> Response {
    let accounts = discover_accounts(mail_root);

    // Find the first account that still has un-triaged messages and
    // process one batch (up to 20 messages).
    for account in &accounts {
        let account_dir = mail_root.join(&account.name);
        match triage::triage_batch(&account_dir, None, 1) {
            Ok(r) if r.triaged > 0 => {
                let flash = url_encode(&format!(
                    "Triaged {} message(s) in {}. {} remaining — click Triage again to continue.",
                    r.triaged, account.name, r.remaining
                ));
                return Response::redirect(&format!("/?flash={flash}"));
            }
            Ok(_) => continue,
            Err(e) => {
                let flash = url_encode(&format!("error: {}: {e:#}", account.name));
                return Response::redirect(&format!("/?flash={flash}"));
            }
        }
    }

    let flash = url_encode("Nothing left to triage.");
    Response::redirect(&format!("/?flash={flash}"))
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

fn apply_prompt_to_message(
    mail_root: &Path,
    prompts_dir: &Path,
    account: &str,
    filename: &str,
    prompt_name: &str,
) -> Result<String> {
    let def = prompts::read_prompt(prompts_dir, prompt_name)?;
    let account_dir = mail_root.join(account);
    let msg_path = account_dir.join("INBOX").join("new").join(filename);
    let raw = std::fs::read(&msg_path)
        .with_context(|| format!("reading {}", msg_path.display()))?;
    let parser = mail_parser::MessageParser::default();
    let parsed = parser.parse(&raw)
        .ok_or_else(|| anyhow::anyhow!("could not parse {filename}"))?;

    let from = parsed.from()
        .and_then(|a| match a {
            mail_parser::Address::List(list) => list.first()
                .and_then(|addr| addr.address.as_deref())
                .map(|s| s.to_string()),
            _ => None,
        })
        .unwrap_or_else(|| "(unknown)".into());
    let subject = parsed.subject().unwrap_or("(no subject)").to_string();
    let body = parsed.body_text(0).unwrap_or_default().to_string();

    let vars = HashMap::from([
        ("from", from.as_str()),
        ("subject", subject.as_str()),
        ("body", body.as_str()),
    ]);
    let rendered = prompts::render_template(&def.template, &vars);
    let response = triage::ollama_generate("gemma4:latest", &rendered)?;
    prompts::write_result(&account_dir, prompt_name, filename, response.trim())?;
    Ok(response)
}

fn build_reports(mail_root: &Path) -> Result<Vec<AccountReport>> {
    let accounts = discover_accounts(mail_root);
    mail_report(&accounts, mail_root)
}

// ---- HTTP shell ----

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        303 => "See Other",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "OK",
    }
}

fn write_response<W: Write>(w: &mut W, resp: &Response) -> std::io::Result<()> {
    write!(
        w,
        "HTTP/1.1 {} {}\r\n",
        resp.status,
        status_text(resp.status)
    )?;
    write!(w, "Content-Type: {}\r\n", resp.content_type)?;
    write!(w, "Content-Length: {}\r\n", resp.body.len())?;
    write!(w, "Connection: close\r\n")?;
    for (k, v) in &resp.extra_headers {
        write!(w, "{k}: {v}\r\n")?;
    }
    w.write_all(b"\r\n")?;
    w.write_all(&resp.body)?;
    Ok(())
}

fn handle_connection(stream: TcpStream, mail_root: &Path, prompts_dir: &Path) -> Result<()> {
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .ok();
    let peer = stream.try_clone()?;
    let mut reader = BufReader::new(peer);
    let resp = match parse_request(&mut reader)? {
        Some(req) => route(&req, mail_root, prompts_dir),
        None => return Ok(()),
    };
    let mut writer = stream;
    write_response(&mut writer, &resp)?;
    writer.flush()?;
    Ok(())
}

const TRIAGE_TEMPLATE: &str = "\
Classify this email as exactly one of: important, actionable, newsletter, spam.
Rules:
- important: personal mail, work mail, anything needing human attention
- actionable: bills, receipts, password resets, verification codes, shipping notifications
- newsletter: marketing, digests, promotions, social media notifications
- spam: unsolicited, scams, phishing

Reply with ONLY the classification. No explanation.

From: {{from}}
Subject: {{subject}}

{{body}}";

const SUMMARIZE_TEMPLATE: &str = "\
Summarize this email in 1-3 sentences. Be concise and capture the key point.

From: {{from}}
Subject: {{subject}}

{{body}}";

fn main() -> Result<()> {
    let humos = humos::humos_dir()?;
    let mail_root: PathBuf = humos.join("mail");
    let prompts_dir: PathBuf = humos.join("prompts");

    // Seed default prompts.
    prompts::seed_prompt(
        &prompts_dir,
        "triage",
        "Classify as important/actionable/newsletter/spam",
        TRIAGE_TEMPLATE,
    )?;
    prompts::seed_prompt(
        &prompts_dir,
        "summarize",
        "Summarize in 1-3 sentences",
        SUMMARIZE_TEMPLATE,
    )?;

    let listener = TcpListener::bind(BIND_ADDR)
        .with_context(|| format!("binding {BIND_ADDR}"))?;
    eprintln!("humos-web listening on http://{BIND_ADDR}");
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(s) => s,
            Err(e) => {
                eprintln!("accept error: {e}");
                continue;
            }
        };
        let mail_root = mail_root.clone();
        let prompts_dir = prompts_dir.clone();
        if let Err(e) = handle_connection(stream, &mail_root, &prompts_dir) {
            eprintln!("connection error: {e}");
        }
    }
    Ok(())
}

// ================= tests =================
#[cfg(test)]
mod tests {
    use super::*;
    use humos::mail::Summary;
    use std::fs;
    use std::io::Cursor;

    fn write_msg(dir: &Path, name: &str, from: &str, subj: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join(name),
            format!("From: {from}\r\nSubject: {subj}\r\n\r\nbody"),
        )
        .unwrap();
    }

    // ---- url_decode ----
    #[test]
    fn url_decode_plain_passthrough() {
        assert_eq!(url_decode("hello"), "hello");
    }
    #[test]
    fn url_decode_percent_and_plus() {
        assert_eq!(url_decode("a+b%20c"), "a b c");
        assert_eq!(url_decode("name%3Dvalue"), "name=value");
    }
    #[test]
    fn url_decode_keeps_invalid_percent_literal() {
        assert_eq!(url_decode("100%zz"), "100%zz");
    }

    // ---- parse_form ----
    #[test]
    fn parse_form_basic() {
        let f = parse_form("account=gmail&filename=abc");
        assert_eq!(f.get("account").unwrap(), "gmail");
        assert_eq!(f.get("filename").unwrap(), "abc");
    }
    #[test]
    fn parse_form_url_encoded_values() {
        let f = parse_form("account=g%40mail&filename=msg%3A2%2CS");
        assert_eq!(f.get("account").unwrap(), "g@mail");
        assert_eq!(f.get("filename").unwrap(), "msg:2,S");
    }
    #[test]
    fn parse_form_empty_body() {
        assert!(parse_form("").is_empty());
    }

    // ---- html_escape ----
    #[test]
    fn html_escape_specials() {
        assert_eq!(
            html_escape("a<b>&\"'c"),
            "a&lt;b&gt;&amp;&quot;&#39;c"
        );
    }
    #[test]
    fn html_escape_passthrough() {
        assert_eq!(html_escape("hello"), "hello");
    }

    // ---- parse_request ----
    #[test]
    fn parse_request_get_no_body() {
        let raw = b"GET / HTTP/1.1\r\nHost: x\r\n\r\n";
        let mut r = Cursor::new(&raw[..]);
        let req = parse_request(&mut r).unwrap().unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/");
        assert!(req.body.is_empty());
    }
    #[test]
    fn parse_request_post_with_body() {
        let raw = b"POST /archive HTTP/1.1\r\nHost: x\r\nContent-Length: 11\r\n\r\nhello world";
        let mut r = Cursor::new(&raw[..]);
        let req = parse_request(&mut r).unwrap().unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/archive");
        assert_eq!(req.body, b"hello world");
    }
    #[test]
    fn parse_request_eof_returns_none() {
        let mut r = Cursor::new(&[][..]);
        assert!(parse_request(&mut r).unwrap().is_none());
    }

    // ---- render_index_html ----
    #[test]
    fn render_index_html_empty_shows_add_account_link() {
        let tmp = tempfile::tempdir().unwrap();
        let html = render_index_html(&[], tmp.path(), None);
        assert!(html.contains("/add-account"));
        assert!(html.contains("Add"));
    }
    #[test]
    fn render_index_html_has_sync_and_triage_buttons() {
        let tmp = tempfile::tempdir().unwrap();
        let html = render_index_html(&[], tmp.path(), None);
        assert!(html.contains("action=\"/sync\""));
        assert!(html.contains("Sync all"));
        assert!(html.contains("action=\"/triage\""));
        assert!(html.contains("Triage"));
    }
    #[test]
    fn render_index_html_shows_flash_message() {
        let tmp = tempfile::tempdir().unwrap();
        let html = render_index_html(&[], tmp.path(), Some("Synced: 5 new message(s)"));
        assert!(html.contains("Synced: 5 new message(s)"));
        assert!(html.contains("flash-ok"));
    }
    #[test]
    fn render_index_html_shows_error_flash() {
        let tmp = tempfile::tempdir().unwrap();
        let html = render_index_html(&[], tmp.path(), Some("error: login failed"));
        assert!(html.contains("flash-err"));
    }
    #[test]
    fn render_index_html_lists_messages_and_archive_buttons() {
        let tmp = tempfile::tempdir().unwrap();
        let report = AccountReport {
            account: "gmail".into(),
            label: "gmail <me@gmail.com>".into(),
            unread_count: 1,
            read_count: 0,
            recent: vec![Summary {
                from: "alice@example.com".into(),
                subject: "Hi <there>".into(),
                filename: "abc".into(),
            }],
        };
        let html = render_index_html(&[report], tmp.path(), None);
        assert!(html.contains("gmail &lt;me@gmail.com&gt;"));
        assert!(html.contains("alice@example.com"));
        assert!(html.contains("Hi &lt;there&gt;"));
        assert!(html.contains("name=\"account\" value=\"gmail\""));
        assert!(html.contains("name=\"filename\" value=\"abc\""));
        assert!(html.contains("action=\"/archive\""));
    }
    #[test]
    fn render_index_html_escapes_evil_filename() {
        let tmp = tempfile::tempdir().unwrap();
        let report = AccountReport {
            account: "gmail".into(),
            label: "gmail".into(),
            unread_count: 1,
            read_count: 0,
            recent: vec![Summary {
                from: "x".into(),
                subject: "y".into(),
                filename: "\"><script>".into(),
            }],
        };
        let html = render_index_html(&[report], tmp.path(), None);
        assert!(!html.contains("<script>"));
        assert!(html.contains("&quot;&gt;&lt;script&gt;"));
    }
    #[test]
    fn render_index_html_shows_triage_label_when_present() {
        let tmp = tempfile::tempdir().unwrap();
        let account_dir = tmp.path().join("gmail");
        fs::create_dir_all(&account_dir).unwrap();
        triage::write_label(&account_dir, "abc", &triage::Label::Important).unwrap();
        let report = AccountReport {
            account: "gmail".into(),
            label: "gmail".into(),
            unread_count: 1,
            read_count: 0,
            recent: vec![Summary {
                from: "x".into(),
                subject: "y".into(),
                filename: "abc".into(),
            }],
        };
        let html = render_index_html(&[report], tmp.path(), None);
        assert!(html.contains("label-important"));
        assert!(html.contains("important"));
    }

    // ---- route ----
    fn req(method: &str, path: &str, body: &str) -> Request {
        Request {
            method: method.into(),
            path: path.into(),
            body: body.as_bytes().to_vec(),
        }
    }

    /// Empty prompts dir for route tests that don't need prompts.
    fn empty_prompts(tmp: &tempfile::TempDir) -> PathBuf {
        let p = tmp.path().join("_prompts");
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn route_get_root_empty_mailroot_returns_200_with_add_link() {
        let tmp = tempfile::tempdir().unwrap();
        let resp = route(&req("GET", "/", ""), tmp.path(), &empty_prompts(&tmp));
        assert_eq!(resp.status, 200);
        assert_eq!(resp.content_type, "text/html; charset=utf-8");
        let body = String::from_utf8(resp.body).unwrap();
        assert!(body.contains("/add-account"));
    }

    #[test]
    fn route_get_root_lists_account_messages() {
        let tmp = tempfile::tempdir().unwrap();
        let new_dir = tmp.path().join("gmail").join("INBOX").join("new");
        write_msg(&new_dir, "abc", "alice@example.com", "Hello");
        let resp = route(&req("GET", "/", ""), tmp.path(), &empty_prompts(&tmp));
        assert_eq!(resp.status, 200);
        let body = String::from_utf8(resp.body).unwrap();
        assert!(body.contains("alice@example.com"));
        assert!(body.contains("Hello"));
        assert!(body.contains("value=\"abc\""));
    }

    #[test]
    fn route_unknown_path_returns_404() {
        let tmp = tempfile::tempdir().unwrap();
        let resp = route(&req("GET", "/nope", ""), tmp.path(), &empty_prompts(&tmp));
        assert_eq!(resp.status, 404);
    }

    #[test]
    fn route_post_archive_moves_file_and_redirects() {
        let tmp = tempfile::tempdir().unwrap();
        let new_dir = tmp.path().join("gmail").join("INBOX").join("new");
        write_msg(&new_dir, "abc", "a@x", "hi");

        let resp = route(
            &req("POST", "/archive", "account=gmail&filename=abc"),
            tmp.path(),
            &empty_prompts(&tmp),
        );
        assert_eq!(resp.status, 303);
        assert!(resp
            .extra_headers
            .iter()
            .any(|(k, v)| k == "Location" && v == "/"));
        assert!(!new_dir.join("abc").exists());
        assert!(tmp
            .path()
            .join("gmail")
            .join("Archive")
            .join("cur")
            .join("abc:2,S")
            .is_file());
    }

    #[test]
    fn route_post_archive_missing_filename_is_400() {
        let tmp = tempfile::tempdir().unwrap();
        let resp = route(&req("POST", "/archive", "account=gmail"), tmp.path(), &empty_prompts(&tmp));
        assert_eq!(resp.status, 400);
    }

    #[test]
    fn route_post_archive_unknown_message_is_400() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("gmail").join("INBOX").join("new")).unwrap();
        let resp = route(
            &req("POST", "/archive", "account=gmail&filename=ghost"),
            tmp.path(),
            &empty_prompts(&tmp),
        );
        assert_eq!(resp.status, 400);
        assert!(String::from_utf8(resp.body).unwrap().contains("archive failed"));
    }

    #[test]
    fn route_get_root_ignores_query_string() {
        let tmp = tempfile::tempdir().unwrap();
        let resp = route(&req("GET", "/?x=1", ""), tmp.path(), &empty_prompts(&tmp));
        assert_eq!(resp.status, 200);
    }

    // ---- write_response ----
    #[test]
    fn write_response_includes_status_headers_and_body() {
        let resp = Response::html(200, "<p>hi</p>");
        let mut buf: Vec<u8> = Vec::new();
        write_response(&mut buf, &resp).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(s.contains("Content-Type: text/html; charset=utf-8\r\n"));
        assert!(s.contains("Content-Length: 9\r\n"));
        assert!(s.ends_with("<p>hi</p>"));
    }

    #[test]
    fn write_response_redirect_emits_location_header() {
        let resp = Response::redirect("/");
        let mut buf: Vec<u8> = Vec::new();
        write_response(&mut buf, &resp).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.starts_with("HTTP/1.1 303 See Other\r\n"));
        assert!(s.contains("Location: /\r\n"));
    }

    // ---- render_add_account_html ----
    #[test]
    fn render_add_account_html_contains_form_fields() {
        let html = render_add_account_html(None);
        assert!(html.contains("name=\"name\""));
        assert!(html.contains("name=\"email\""));
        assert!(html.contains("name=\"password\""));
        assert!(html.contains("action=\"/add-account\""));
        assert!(html.contains("myaccount.google.com/apppasswords"));
    }
    #[test]
    fn render_add_account_html_shows_error() {
        let html = render_add_account_html(Some("bad name"));
        assert!(html.contains("bad name"));
        assert!(html.contains("flash-err"));
    }

    // ---- route: add-account ----
    #[test]
    fn route_get_add_account_returns_200_form() {
        let tmp = tempfile::tempdir().unwrap();
        let resp = route(&req("GET", "/add-account", ""), tmp.path(), &empty_prompts(&tmp));
        assert_eq!(resp.status, 200);
        let body = String::from_utf8(resp.body).unwrap();
        assert!(body.contains("name=\"email\""));
    }

    #[test]
    fn route_post_add_account_missing_name_returns_400() {
        let tmp = tempfile::tempdir().unwrap();
        let resp = route(
            &req("POST", "/add-account", "email=x%40gmail.com&password=abc"),
            tmp.path(),
            &empty_prompts(&tmp),
        );
        assert_eq!(resp.status, 400);
    }

    #[test]
    fn route_post_add_account_unknown_provider_returns_400() {
        let tmp = tempfile::tempdir().unwrap();
        let resp = route(
            &req(
                "POST",
                "/add-account",
                "name=test&email=x%40unknown.org&password=abc",
            ),
            tmp.path(),
            &empty_prompts(&tmp),
        );
        assert_eq!(resp.status, 400);
        let body = String::from_utf8(resp.body).unwrap();
        assert!(body.contains("Unknown email provider"));
    }

    // ---- url_encode ----
    #[test]
    fn url_encode_plain() {
        assert_eq!(url_encode("hello"), "hello");
    }
    #[test]
    fn url_encode_spaces_and_specials() {
        let e = url_encode("a b&c");
        assert_eq!(e, "a%20b%26c");
    }

    // ---- route: flash via query string ----
    #[test]
    fn route_get_root_with_flash_shows_flash() {
        let tmp = tempfile::tempdir().unwrap();
        let resp = route(&req("GET", "/?flash=hello", ""), tmp.path(), &empty_prompts(&tmp));
        assert_eq!(resp.status, 200);
        let body = String::from_utf8(resp.body).unwrap();
        assert!(body.contains("hello"));
    }
}
