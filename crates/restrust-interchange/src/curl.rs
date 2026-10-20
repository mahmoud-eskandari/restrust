//! cURL command import/export.
//!
//! Import parses a shell-like command line (own tokenizer: single quotes,
//! double quotes with backslash escapes, backslash line continuations — no
//! external dependencies) and maps `-X`, `-H`, `-d/--data*`, `-F`, `-u`, `-b`
//! and friends onto a [`RequestDef`]. Flags restrust cannot model (e.g.
//! `-k`, `-L`, `--compressed`) are ignored and noted in the request
//! description. Export renders a multi-line, safely single-quoted command.

use restrust_core::model::{
    new_id, ApiKeyIn, AuthConfig, AuthKind, BodyKind, HttpMethod, KeyValue, RequestBody, RequestDef,
};

use crate::InterchangeError;

// --- public API ---------------------------------------------------------------

/// Parses a cURL command into a [`RequestDef`]. `GET` is the default method;
/// data/form flags imply `POST` unless `-X`/`-G` say otherwise. The first
/// non-flag argument is the URL.
pub fn import_curl(cmd: &str) -> Result<RequestDef, InterchangeError> {
    let tokens = tokenize(cmd)?;
    if tokens.is_empty() {
        return Err(InterchangeError::BadInput("empty cURL command".to_string()));
    }
    let mut i = 0;
    let first = tokens[0].as_str();
    if first == "curl" || first.ends_with("/curl") || first == "curl.exe" {
        i = 1;
    }

    let mut method: Option<HttpMethod> = None;
    let mut url: Option<String> = None;
    let mut headers: Vec<KeyValue> = Vec::new();
    let mut data_parts: Vec<String> = Vec::new();
    let mut form_parts: Vec<KeyValue> = Vec::new();
    let mut basic: Option<(String, String)> = None;
    let mut cookie: Option<String> = None;
    let mut get_flag = false;
    let mut json_flag = false;
    let mut ignored: Vec<String> = Vec::new();

    while i < tokens.len() {
        let token = tokens[i].clone();
        i += 1;
        // Long flags may carry an inline value: --data-raw=...
        let (flag, inline): (&str, Option<String>) = if token.starts_with("--") {
            match token.split_once('=') {
                Some((name, value)) => (name, Some(value.to_string())),
                None => (token.as_str(), None),
            }
        } else {
            (token.as_str(), None)
        };
        match flag {
            "-X" | "--request" => {
                let v = take_value(&tokens, &mut i, inline, flag)?;
                method = Some(v.parse().map_err(|_| {
                    InterchangeError::BadInput(format!("unsupported HTTP method '{v}'"))
                })?);
            }
            "-H" | "--header" => {
                let v = take_value(&tokens, &mut i, inline, flag)?;
                match parse_header_row(&v) {
                    Some(row) => headers.push(row),
                    None => ignored.push(format!("malformed header '{v}'")),
                }
            }
            "-d" | "--data" | "--data-raw" | "--data-ascii" | "--data-binary" => {
                data_parts.push(take_value(&tokens, &mut i, inline, flag)?);
            }
            "--json" => {
                json_flag = true;
                data_parts.push(take_value(&tokens, &mut i, inline, flag)?);
            }
            "-F" | "--form" => {
                let v = take_value(&tokens, &mut i, inline, flag)?;
                match v.split_once('=') {
                    Some((k, v)) => {
                        // `-F f=@a.png` file references keep the name only.
                        let v = v.strip_prefix('@').unwrap_or(v).to_string();
                        form_parts.push(KeyValue::new(k, v));
                    }
                    None => ignored.push(format!("malformed form part '{v}'")),
                }
            }
            "-u" | "--user" => {
                let v = take_value(&tokens, &mut i, inline, flag)?;
                let (u, p) = v.split_once(':').unwrap_or((v.as_str(), ""));
                basic = Some((u.to_string(), p.to_string()));
            }
            "-b" | "--cookie" => cookie = Some(take_value(&tokens, &mut i, inline, flag)?),
            "-A" | "--user-agent" => {
                let v = take_value(&tokens, &mut i, inline, flag)?;
                headers.push(KeyValue::new("User-Agent", v));
            }
            "-e" | "--referer" => {
                let v = take_value(&tokens, &mut i, inline, flag)?;
                headers.push(KeyValue::new("Referer", v));
            }
            "--url" => url = Some(take_value(&tokens, &mut i, inline, flag)?),
            "-G" | "--get" => get_flag = true,
            // boolean flags that restrust cannot model: ignored, noted below
            "-k" | "--insecure" | "-L" | "--location" | "-s" | "--silent" | "-S"
            | "--show-error" | "-i" | "--include" | "-v" | "--verbose" | "--compressed" | "-f"
            | "--fail" | "-#" | "--progress-bar" | "-4" | "-6" | "--http1.1" | "--http2"
            | "--http3" | "-N" | "--no-buffer" | "-g" | "--globoff" => {
                ignored.push(flag.to_string());
            }
            // value-taking flags that restrust cannot model: consume + note
            "-o" | "--output" | "-m" | "--max-time" | "--connect-timeout" | "-x" | "--proxy"
            | "--cacert" | "--capath" | "--cert" | "--key" | "--cert-type" | "--key-type"
            | "-w" | "--write-out" | "-D" | "--dump-header" | "-c" | "--cookie-jar" | "--retry"
            | "-t" | "--telnet-option" | "--trace" | "--trace-ascii" | "--trace-time"
            | "--resolve" | "--limit-rate" | "-y" | "--speed-limit" | "-Y" | "--speed-time"
            | "-z" | "--time-cond" | "--ciphers" | "--tls-max" => {
                let _ = take_value(&tokens, &mut i, inline, flag)?;
                ignored.push(flag.to_string());
            }
            _ => {
                if token.starts_with('-') && token != "-" {
                    // Unknown flag: assume boolean, note it.
                    ignored.push(token.clone());
                } else if url.is_none() {
                    url = Some(token.clone());
                } else {
                    ignored.push(format!("extra argument '{token}'"));
                }
            }
        }
    }

    let mut url =
        url.ok_or_else(|| InterchangeError::BadInput("no URL found in cURL command".to_string()))?;
    // -G moves the data to the query string.
    if get_flag && !data_parts.is_empty() {
        let sep = if url.contains('?') { '&' } else { '?' };
        url = format!("{url}{sep}{}", data_parts.join("&"));
        data_parts.clear();
    }
    let effective_method = method.unwrap_or({
        if !data_parts.is_empty() || !form_parts.is_empty() {
            HttpMethod::Post
        } else {
            HttpMethod::Get
        }
    });

    let content_type = headers
        .iter()
        .find(|h| h.key.eq_ignore_ascii_case("content-type"))
        .map(|h| h.value.clone());
    let body = if !form_parts.is_empty() {
        if !data_parts.is_empty() {
            ignored.push("both -F and -d given; form parts kept, data dropped".to_string());
        }
        RequestBody {
            kind: BodyKind::Multipart,
            raw: None,
            rows: form_parts,
        }
    } else if !data_parts.is_empty() {
        let mime = content_type.clone().unwrap_or_else(|| {
            if json_flag {
                "application/json".to_string()
            } else {
                "application/x-www-form-urlencoded".to_string()
            }
        });
        RequestBody {
            kind: BodyKind::Raw { mime },
            raw: Some(data_parts.join("&")),
            rows: Vec::new(),
        }
    } else {
        RequestBody::default()
    };

    if let Some(cookie) = cookie {
        if !headers.iter().any(|h| h.key.eq_ignore_ascii_case("cookie")) {
            headers.push(KeyValue::new("Cookie", cookie));
        }
    }

    let description = (!ignored.is_empty()).then(|| {
        format!(
            "Imported from cURL. Ignored options: {}.",
            ignored.join(", ")
        )
    });

    Ok(RequestDef {
        id: new_id(),
        name: "Imported from cURL".to_string(),
        description,
        method: effective_method,
        url,
        query: Vec::new(),
        headers,
        body,
        auth: basic.map(|(username, password)| AuthConfig {
            kind: AuthKind::Basic { username, password },
        }),
    })
}

/// Renders a [`RequestDef`] as a multi-line shell command with single-quoted
/// arguments (properly escaped). `-X` is emitted for non-GET methods,
/// enabled headers as `-H`, raw bodies as `--data-raw`, multipart rows as
/// `-F`, binary bodies as `--data-binary @path`, and basic auth as `--user`.
pub fn export_curl(r: &RequestDef) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("curl {}", shell_quote(&url_with_query(r))));
    if r.method != HttpMethod::Get {
        lines.push(format!("-X {}", r.method));
    }

    let has_content_type = r
        .headers
        .iter()
        .any(|h| h.enabled && h.key.eq_ignore_ascii_case("content-type"));
    let mut header_lines: Vec<String> = r
        .headers
        .iter()
        .filter(|h| h.enabled)
        .map(|h| format!("-H {}", shell_quote(&format!("{}: {}", h.key, h.value))))
        .collect();
    match r.auth.as_ref().map(|a| &a.kind) {
        Some(AuthKind::Bearer { token }) => {
            if !has_header(&r.headers, "Authorization") {
                header_lines.push(format!(
                    "-H {}",
                    shell_quote(&format!("Authorization: Bearer {token}"))
                ));
            }
        }
        Some(AuthKind::ApiKey { key, value, add_to })
            if *add_to == ApiKeyIn::Header && !has_header(&r.headers, key) =>
        {
            header_lines.push(format!("-H {}", shell_quote(&format!("{key}: {value}"))));
        }
        _ => {}
    }
    lines.extend(header_lines);

    match &r.body.kind {
        BodyKind::None => {}
        BodyKind::Raw { mime } => {
            if !has_content_type {
                lines.push(format!(
                    "-H {}",
                    shell_quote(&format!("Content-Type: {mime}"))
                ));
            }
            lines.push(format!(
                "--data-raw {}",
                shell_quote(r.body.raw.as_deref().unwrap_or(""))
            ));
        }
        BodyKind::UrlEncoded => {
            if !has_content_type {
                lines.push(format!(
                    "-H {}",
                    shell_quote("Content-Type: application/x-www-form-urlencoded")
                ));
            }
            let data = r
                .body
                .rows
                .iter()
                .filter(|kv| kv.enabled)
                .map(|kv| format!("{}={}", kv.key, kv.value))
                .collect::<Vec<_>>()
                .join("&");
            lines.push(format!("--data-raw {}", shell_quote(&data)));
        }
        BodyKind::Multipart => {
            for kv in r.body.rows.iter().filter(|kv| kv.enabled) {
                lines.push(format!(
                    "-F {}",
                    shell_quote(&format!("{}={}", kv.key, kv.value))
                ));
            }
        }
        BodyKind::Binary { file_path } => {
            lines.push(format!(
                "--data-binary {}",
                shell_quote(&format!("@{file_path}"))
            ));
        }
    }

    if let Some(AuthKind::Basic { username, password }) = r.auth.as_ref().map(|a| &a.kind) {
        lines.push(format!(
            "--user {}",
            shell_quote(&format!("{username}:{password}"))
        ));
    }

    let mut out = String::new();
    for (idx, line) in lines.iter().enumerate() {
        if idx == 0 {
            out.push_str(line);
        } else {
            out.push_str("\n  ");
            out.push_str(line);
        }
        if idx + 1 < lines.len() {
            out.push_str(" \\");
        }
    }
    out
}

// --- helpers --------------------------------------------------------------------

/// Single-quotes a string for safe shell use (`'` becomes `'\''`).
pub(crate) fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

fn has_header(headers: &[KeyValue], name: &str) -> bool {
    headers
        .iter()
        .any(|h| h.enabled && h.key.eq_ignore_ascii_case(name))
}

fn url_with_query(r: &RequestDef) -> String {
    let mut pairs: Vec<String> = r
        .query
        .iter()
        .filter(|q| q.enabled)
        .map(|q| format!("{}={}", q.key, q.value))
        .collect();
    if let Some(AuthKind::ApiKey { key, value, add_to }) = r.auth.as_ref().map(|a| &a.kind) {
        if *add_to == ApiKeyIn::Query && !pairs.iter().any(|p| p.starts_with(&format!("{key}="))) {
            pairs.push(format!("{key}={value}"));
        }
    }
    if pairs.is_empty() {
        return r.url.clone();
    }
    let sep = if r.url.contains('?') { '&' } else { '?' };
    format!("{}{sep}{}", r.url, pairs.join("&"))
}

fn parse_header_row(raw: &str) -> Option<KeyValue> {
    let raw = raw.trim();
    let (name, value) = raw.split_once(':')?;
    Some(KeyValue {
        key: name.trim().to_string(),
        value: value.trim().to_string(),
        enabled: true,
        description: None,
    })
}

fn take_value(
    tokens: &[String],
    i: &mut usize,
    inline: Option<String>,
    flag: &str,
) -> Result<String, InterchangeError> {
    if let Some(v) = inline {
        return Ok(v);
    }
    match tokens.get(*i) {
        Some(v) => {
            *i += 1;
            Ok(v.clone())
        }
        None => Err(InterchangeError::BadInput(format!(
            "cURL flag '{flag}' expects a value"
        ))),
    }
}

/// Shell-like tokenizer: splits on whitespace, honors `'...'` (literal) and
/// `"..."` (with `\"` `\\` `\$` `` \` `` escapes), treats a backslash before
/// a newline as a line continuation, and lets a backslash escape any other
/// character outside quotes.
fn tokenize(cmd: &str) -> Result<Vec<String>, InterchangeError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut chars = cmd.chars().peekable();

    macro_rules! unterminated {
        ($q:expr) => {
            return Err(InterchangeError::BadInput(format!(
                "unterminated {} quote in cURL command",
                $q
            )))
        };
    }

    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                has_token = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => current.push(ch),
                        None => unterminated!("single"),
                    }
                }
            }
            '"' => {
                has_token = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(e) if matches!(e, '"' | '\\' | '$' | '`') => current.push(e),
                            Some('\n') => {} // line continuation inside quotes
                            Some(other) => {
                                current.push('\\');
                                current.push(other);
                            }
                            None => unterminated!("double"),
                        },
                        Some(ch) => current.push(ch),
                        None => unterminated!("double"),
                    }
                }
            }
            '\\' => match chars.next() {
                Some('\n') => {} // line continuation
                Some(other) => {
                    current.push(other);
                    has_token = true;
                }
                None => {
                    current.push('\\');
                    has_token = true;
                }
            },
            c if c.is_whitespace() => {
                if has_token {
                    tokens.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            c => {
                current.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        tokens.push(current);
    }
    Ok(tokens)
}
