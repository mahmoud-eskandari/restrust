//! Code-snippet generation for saved requests (pure string generation, no
//! execution). Targets: cURL, Rust (reqwest), Python (requests), JavaScript
//! (fetch) and Go (net/http). Auth renders through each library's idiomatic
//! option (`.basic_auth`/`req.SetBasicAuth`/`auth=(...)`/`btoa`) where one
//! exists, and as an explicit header otherwise.

use restrust_core::model::{ApiKeyIn, AuthKind, BodyKind, HttpMethod, RequestDef};

use crate::curl;

/// The snippet language/toolchain to generate for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Target {
    Curl,
    RustReqwest,
    PythonRequests,
    JsFetch,
    GoNetHttp,
}

/// Generates a code snippet for `r` in the given target.
pub fn generate(r: &RequestDef, t: Target) -> String {
    match t {
        Target::Curl => curl::export_curl(r),
        Target::RustReqwest => rust_reqwest(r),
        Target::PythonRequests => python_requests(r),
        Target::JsFetch => js_fetch(r),
        Target::GoNetHttp => go_net_http(r),
    }
}

// --- shared preparation -----------------------------------------------------------

/// A request flattened into what each renderer needs: query rows and
/// query-located API keys are baked into the URL, auth is resolved to either
/// library-native `bearer`/`basic` handles or extra header rows.
struct Prepared {
    method: HttpMethod,
    url: String,
    headers: Vec<(String, String)>,
    bearer: Option<String>,
    basic: Option<(String, String)>,
    body: PrepBody,
}

enum PrepBody {
    None,
    Raw { text: String },
    UrlEncoded(Vec<(String, String)>),
    Multipart(Vec<(String, String)>),
    Binary(String),
}

fn prepare(r: &RequestDef) -> Prepared {
    let mut headers: Vec<(String, String)> = r
        .headers
        .iter()
        .filter(|h| h.enabled)
        .map(|h| (h.key.clone(), h.value.clone()))
        .collect();
    let mut query: Vec<(String, String)> = r
        .query
        .iter()
        .filter(|q| q.enabled)
        .map(|q| (q.key.clone(), q.value.clone()))
        .collect();
    let mut bearer = None;
    let mut basic = None;
    match r.auth.as_ref().map(|a| &a.kind) {
        Some(AuthKind::Bearer { token }) => {
            if !headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            {
                bearer = Some(token.clone());
            }
        }
        Some(AuthKind::Basic { username, password }) => {
            basic = Some((username.clone(), password.clone()));
        }
        Some(AuthKind::ApiKey { key, value, add_to }) => match add_to {
            ApiKeyIn::Header => {
                if !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(key)) {
                    headers.push((key.clone(), value.clone()));
                }
            }
            ApiKeyIn::Query => {
                if !query.iter().any(|(k, _)| k == key) {
                    query.push((key.clone(), value.clone()));
                }
            }
        },
        _ => {}
    }
    let url = if query.is_empty() {
        r.url.clone()
    } else {
        let pairs = query
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let sep = if r.url.contains('?') { '&' } else { '?' };
        format!("{}{sep}{pairs}", r.url)
    };
    let body = match &r.body.kind {
        BodyKind::None => PrepBody::None,
        BodyKind::Raw { .. } => PrepBody::Raw {
            text: r.body.raw.clone().unwrap_or_default(),
        },
        BodyKind::UrlEncoded => PrepBody::UrlEncoded(rows(r)),
        BodyKind::Multipart => PrepBody::Multipart(rows(r)),
        BodyKind::Binary { file_path } => PrepBody::Binary(file_path.clone()),
    };
    Prepared {
        method: r.method,
        url,
        headers,
        bearer,
        basic,
        body,
    }
}

fn rows(r: &RequestDef) -> Vec<(String, String)> {
    r.body
        .rows
        .iter()
        .filter(|kv| kv.enabled)
        .map(|kv| (kv.key.clone(), kv.value.clone()))
        .collect()
}

// --- string helpers ----------------------------------------------------------------

fn dquote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn squote(s: &str) -> String {
    // Python/JS single-quoted literals; also escapes newlines/tabs.
    format!(
        "'{}'",
        s.replace('\\', "\\\\")
            .replace('\'', "\\'")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    )
}

/// Rust raw string with escalating `#` delimiters (`r#"..."#`).
fn rust_raw(text: &str) -> String {
    let mut hashes = 1;
    while text.contains(&format!("\"{}", "#".repeat(hashes))) {
        hashes += 1;
    }
    let h = "#".repeat(hashes);
    format!("r{h}\"{text}\"{h}")
}

/// Go literal: backtick raw string when possible, escaped double quotes
/// otherwise (raw strings cannot contain backticks).
fn go_str(s: &str) -> String {
    if !s.contains('`') && !s.contains('\r') {
        format!("`{s}`")
    } else {
        dquote(s)
    }
}

// --- Rust / reqwest ------------------------------------------------------------------

fn rust_reqwest(r: &RequestDef) -> String {
    let p = prepare(r);
    let mut s = String::from("// reqwest 0.13 (async)\nlet client = reqwest::Client::new();\n");
    // reqwest has convenience builders for all methods except OPTIONS.
    if p.method == HttpMethod::Options {
        s.push_str(&format!(
            "let response = client\n    .request(reqwest::Method::OPTIONS, {})\n",
            dquote(&p.url)
        ));
    } else {
        s.push_str(&format!(
            "let response = client\n    .{}({})\n",
            p.method.as_str().to_ascii_lowercase(),
            dquote(&p.url)
        ));
    }
    for (k, v) in &p.headers {
        s.push_str(&format!("    .header({}, {})\n", dquote(k), dquote(v)));
    }
    if let Some(token) = &p.bearer {
        s.push_str(&format!("    .bearer_auth({})\n", dquote(token)));
    }
    if let Some((u, pw)) = &p.basic {
        s.push_str(&format!(
            "    .basic_auth({}, Some({}))\n",
            dquote(u),
            dquote(pw)
        ));
    }
    match &p.body {
        PrepBody::None => {}
        PrepBody::Raw { text, .. } => {
            s.push_str(&format!("    .body({})\n", rust_raw(text)));
        }
        PrepBody::UrlEncoded(pairs) => {
            s.push_str("    .form(&[\n");
            for (k, v) in pairs {
                s.push_str(&format!("        ({}, {}),\n", dquote(k), dquote(v)));
            }
            s.push_str("    ])\n");
        }
        PrepBody::Multipart(pairs) => {
            s.push_str("    .multipart(\n        reqwest::multipart::Form::new()\n");
            for (k, v) in pairs {
                s.push_str(&format!(
                    "            .text({}, {})\n",
                    dquote(k),
                    dquote(v)
                ));
            }
            s.push_str("    )\n");
        }
        PrepBody::Binary(path) => {
            s.push_str(&format!("    .body(std::fs::read({})?)\n", dquote(path)));
        }
    }
    s.push_str(
        "    .send()\n    .await?;\n\nlet status = response.status();\nlet body = response.text().await?;\n",
    );
    s
}

// --- Python / requests ----------------------------------------------------------------

fn python_requests(r: &RequestDef) -> String {
    let p = prepare(r);
    let mut headers = p.headers.clone();
    if let Some(token) = &p.bearer {
        headers.push(("Authorization".to_string(), format!("Bearer {token}")));
    }
    let mut s = String::from("import requests\n\n");
    s.push_str(&format!("url = {}\n", squote(&p.url)));
    if !headers.is_empty() {
        s.push_str("headers = {\n");
        for (k, v) in &headers {
            s.push_str(&format!("    {}: {},\n", squote(k), squote(v)));
        }
        s.push_str("}\n");
    }
    if let Some((u, pw)) = &p.basic {
        s.push_str(&format!("auth = ({}, {})\n", squote(u), squote(pw)));
    }
    let mut args: Vec<String> = Vec::new();
    if !headers.is_empty() {
        args.push("headers=headers".to_string());
    }
    if p.basic.is_some() {
        args.push("auth=auth".to_string());
    }
    match &p.body {
        PrepBody::None => {}
        PrepBody::Raw { text, .. } => args.push(format!("data={}", squote(text))),
        PrepBody::UrlEncoded(pairs) => {
            let items = pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", squote(k), squote(v)))
                .collect::<Vec<_>>()
                .join(", ");
            args.push(format!("data={{{items}}}"));
        }
        PrepBody::Multipart(pairs) => {
            let items = pairs
                .iter()
                .map(|(k, v)| format!("{}: (None, {})", squote(k), squote(v)))
                .collect::<Vec<_>>()
                .join(", ");
            args.push(format!("files={{{items}}}"));
        }
        PrepBody::Binary(path) => args.push(format!("data=open({}, 'rb')", squote(path))),
    }
    let call = if args.is_empty() {
        String::new()
    } else {
        format!(", {}", args.join(", "))
    };
    s.push_str(&format!(
        "\nresponse = requests.{}(url{})\n",
        p.method.as_str().to_ascii_lowercase(),
        call
    ));
    s.push_str("print(response.status_code)\nprint(response.text)\n");
    s
}

// --- JavaScript / fetch -----------------------------------------------------------------

fn js_fetch(r: &RequestDef) -> String {
    let p = prepare(r);
    let mut headers = p.headers.clone();
    if let Some(token) = &p.bearer {
        headers.push(("Authorization".to_string(), format!("Bearer {token}")));
    }
    // fetch has no native basic-auth option; emit the header expression.
    let basic_header = p.basic.as_ref().and_then(|(u, pw)| {
        if headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("authorization"))
        {
            None
        } else {
            Some(format!("\"Basic \" + btoa(\"{u}:{pw}\")"))
        }
    });
    let mut s = String::new();
    if let PrepBody::Multipart(pairs) = &p.body {
        s.push_str("const form = new FormData();\n");
        for (k, v) in pairs {
            s.push_str(&format!("form.append({}, {});\n", squote(k), squote(v)));
        }
        s.push('\n');
    }
    if let PrepBody::Binary(path) = &p.body {
        s.push_str(&format!(
            "// body: binary file {} (not rendered in this snippet)\n",
            squote(path)
        ));
    }
    s.push_str(&format!(
        "const response = await fetch({}, {{\n",
        squote(&p.url)
    ));
    s.push_str(&format!("  method: \"{}\",\n", p.method.as_str()));
    if !headers.is_empty() || basic_header.is_some() {
        s.push_str("  headers: {\n");
        for (k, v) in &headers {
            s.push_str(&format!("    {}: {},\n", squote(k), squote(v)));
        }
        if let Some(expr) = basic_header {
            s.push_str(&format!("    \"Authorization\": {expr},\n"));
        }
        s.push_str("  },\n");
    }
    match &p.body {
        PrepBody::Raw { text, .. } => {
            s.push_str(&format!("  body: {},\n", squote(text)));
        }
        PrepBody::UrlEncoded(pairs) => {
            let joined = pairs
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            s.push_str(&format!("  body: {},\n", squote(&joined)));
        }
        PrepBody::Multipart(_) => s.push_str("  body: form,\n"),
        PrepBody::None | PrepBody::Binary(_) => {}
    }
    s.push_str("});\n\nif (!response.ok) {\n  throw new Error(`HTTP ${response.status}: ${await response.text()}`);\n}\nconsole.log(await response.text());\n");
    s
}

// --- Go / net/http ------------------------------------------------------------------------

fn go_net_http(r: &RequestDef) -> String {
    let p = prepare(r);
    let needs_strings = !matches!(
        p.body,
        PrepBody::None | PrepBody::Binary(_) | PrepBody::Multipart(_)
    );
    let needs_binary = matches!(p.body, PrepBody::Binary(_));
    let multipart_note = matches!(p.body, PrepBody::Multipart(_));

    let mut s = String::from("package main\n\nimport (\n");
    if needs_binary {
        s.push_str("\t\"bytes\"\n");
    }
    s.push_str("\t\"fmt\"\n\t\"io\"\n\t\"net/http\"\n");
    if needs_binary {
        s.push_str("\t\"os\"\n");
    }
    if needs_strings {
        s.push_str("\t\"strings\"\n");
    }
    s.push_str(")\n\nfunc main() {\n");

    if multipart_note {
        let fields = match &p.body {
            PrepBody::Multipart(pairs) => pairs
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join(", "),
            _ => String::new(),
        };
        s.push_str(&format!(
            "\t// multipart form data is not rendered in this snippet (fields: {fields})\n"
        ));
    }

    let new_request_body = match &p.body {
        PrepBody::None | PrepBody::Multipart(_) => "nil".to_string(),
        PrepBody::Raw { text, .. } => format!("strings.NewReader({})", go_str(text)),
        PrepBody::UrlEncoded(pairs) => {
            let joined = pairs
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            format!("strings.NewReader({})", go_str(&joined))
        }
        PrepBody::Binary(path) => {
            s.push_str(&format!("\tdata, err := os.ReadFile({})\n", dquote(path)));
            s.push_str("\tif err != nil {\n\t\tpanic(err)\n\t}\n");
            "bytes.NewReader(data)".to_string()
        }
    };
    s.push_str(&format!(
        "\treq, err := http.NewRequest(http.Method{}, {}, {})\n",
        go_method_suffix(p.method),
        dquote(&p.url),
        new_request_body
    ));
    s.push_str("\tif err != nil {\n\t\tpanic(err)\n\t}\n");
    for (k, v) in &p.headers {
        s.push_str(&format!("\treq.Header.Set({}, {})\n", dquote(k), dquote(v)));
    }
    if let Some(token) = &p.bearer {
        s.push_str(&format!(
            "\treq.Header.Add({}, {})\n",
            dquote("Authorization"),
            dquote(&format!("Bearer {token}"))
        ));
    }
    if let Some((u, pw)) = &p.basic {
        s.push_str(&format!(
            "\treq.SetBasicAuth({}, {})\n",
            dquote(u),
            dquote(pw)
        ));
    }
    s.push_str(
        "\n\tres, err := http.DefaultClient.Do(req)\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\tdefer res.Body.Close()\n\n\tbody, err := io.ReadAll(res.Body)\n\tif err != nil {\n\t\tpanic(err)\n\t}\n\tfmt.Println(res.Status)\n\tfmt.Println(string(body))\n}\n",
    );
    s
}

/// The Go `http.MethodXxx` constant suffix for a method.
fn go_method_suffix(method: HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get => "Get",
        HttpMethod::Post => "Post",
        HttpMethod::Put => "Put",
        HttpMethod::Patch => "Patch",
        HttpMethod::Delete => "Delete",
        HttpMethod::Head => "Head",
        HttpMethod::Options => "Options",
    }
}
