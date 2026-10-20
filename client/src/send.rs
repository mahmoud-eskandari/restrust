//! Request execution: turning a [`RequestDef`] plus resolved auth and variable
//! scopes into an [`ehttp::Request`], and summarizing responses.

use restrust_core::model::{AuthKind, BodyKind, HttpMethod, KeyValue, RequestDef, ResponseSummary};
use restrust_core::vars::interpolate;
use restrust_core::workspace_doc::WorkspaceDoc;

/// ehttp method for a domain method.
fn ehttp_method(method: HttpMethod) -> ehttp::Method {
    match method {
        HttpMethod::Get => ehttp::Method::GET,
        HttpMethod::Post => ehttp::Method::POST,
        HttpMethod::Put => ehttp::Method::PUT,
        HttpMethod::Patch => ehttp::Method::PATCH,
        HttpMethod::Delete => ehttp::Method::DELETE,
        HttpMethod::Head => ehttp::Method::HEAD,
        HttpMethod::Options => ehttp::Method::OPTIONS,
    }
}

/// Builds the interpolation scopes for a request: environment, then collection
/// variables, then globals (core `vars::interpolate` gives earlier scopes
/// precedence). Request-local overrides are not a v1 feature.
pub fn resolved_scopes<'a>(
    doc: &'a WorkspaceDoc,
    collection: Option<&'a restrust_core::model::Collection>,
) -> Vec<&'a [KeyValue]> {
    let mut scopes = Vec::with_capacity(3);
    if let Some(env_name) = doc.settings.active_environment.as_deref() {
        if let Some(env) = doc.environments.iter().find(|e| e.name == env_name) {
            scopes.push(env.values.as_slice());
        }
    }
    if let Some(collection) = collection {
        scopes.push(collection.variables.as_slice());
    }
    scopes.push(doc.globals.as_slice());
    scopes
}

/// Percent-encodes a query key or value component.
pub fn encode_query_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Standard-alphabet base64 (needed for Basic auth; no extra dependency).
pub fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(ALPHABET[(n >> 18 & 63) as usize] as char);
        out.push(ALPHABET[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Appends enabled, interpolated query rows to a URL.
fn apply_query_rows(url: &str, rows: &[KeyValue], scopes: &[&[KeyValue]]) -> String {
    let pairs: Vec<String> = rows
        .iter()
        .filter(|row| row.enabled && !row.key.trim().is_empty())
        .map(|row| {
            format!(
                "{}={}",
                encode_query_component(&interpolate(&row.key, scopes)),
                encode_query_component(&interpolate(&row.value, scopes))
            )
        })
        .collect();
    if pairs.is_empty() {
        url.to_string()
    } else if url.contains('?') {
        format!("{url}&{}", pairs.join("&"))
    } else {
        format!("{url}?{}", pairs.join("&"))
    }
}

/// Serializes a multipart body, returning the boundary used alongside it.
fn multipart_body(rows: &[KeyValue], scopes: &[&[KeyValue]]) -> (String, Vec<u8>) {
    let boundary = format!("restrust-{}", restrust_core::model::new_id());
    let mut body = Vec::new();
    for row in rows
        .iter()
        .filter(|r| r.enabled && !r.key.trim().is_empty())
    {
        let key = interpolate(&row.key, scopes);
        let value = interpolate(&row.value, scopes);
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{key}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (boundary, body)
}

/// Builds the outgoing HTTP request from a request definition with resolved
/// auth and interpolation scopes. Returns `Err` with an actionable message for
/// unsupported cases (binary bodies on wasm, unreadable files).
pub fn build_request(
    def: &RequestDef,
    auth: &AuthKind,
    scopes: &[&[KeyValue]],
) -> Result<ehttp::Request, String> {
    let mut url = interpolate(&def.url, scopes);
    if url.trim().is_empty() {
        return Err("the URL is empty".to_string());
    }
    if !url.contains("://") {
        url = format!("https://{url}");
    }

    let mut headers: Vec<(String, String)> = Vec::new();
    let mut query_extra: Vec<(String, String)> = Vec::new();
    for row in def
        .headers
        .iter()
        .filter(|r| r.enabled && !r.key.trim().is_empty())
    {
        headers.push((
            interpolate(&row.key, scopes),
            interpolate(&row.value, scopes),
        ));
    }

    // Resolved auth (nearest defined scope wins — resolved by the caller).
    match auth {
        AuthKind::None => {}
        AuthKind::Bearer { token } => {
            let token = interpolate(token, scopes);
            if !token.is_empty() {
                headers.push(("Authorization".into(), format!("Bearer {token}")));
            }
        }
        AuthKind::Basic { username, password } => {
            let raw = format!(
                "{}:{}",
                interpolate(username, scopes),
                interpolate(password, scopes)
            );
            headers.push((
                "Authorization".into(),
                format!("Basic {}", base64_encode(raw.as_bytes())),
            ));
        }
        AuthKind::ApiKey { key, value, add_to } => {
            let key = interpolate(key, scopes);
            let value = interpolate(value, scopes);
            if !key.is_empty() {
                match add_to {
                    restrust_core::model::ApiKeyIn::Header => {
                        headers.push((key, value));
                    }
                    restrust_core::model::ApiKeyIn::Query => {
                        query_extra.push((key, value));
                    }
                }
            }
        }
    }

    let mut url = apply_query_rows(&url, &def.query, scopes);
    for (key, value) in query_extra {
        let sep = if url.contains('?') { '&' } else { '?' };
        url.push(sep);
        url.push_str(&encode_query_component(&key));
        url.push('=');
        url.push_str(&encode_query_component(&value));
    }

    let mut body: Vec<u8> = Vec::new();
    match &def.body.kind {
        BodyKind::None => {}
        BodyKind::Raw { mime } => {
            let text = def.body.raw.clone().unwrap_or_default();
            body = interpolate(&text, scopes).into_bytes();
            if !mime.is_empty()
                && !headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            {
                headers.push(("Content-Type".into(), mime.clone()));
            }
        }
        BodyKind::UrlEncoded => {
            let pairs: Vec<String> = def
                .body
                .rows
                .iter()
                .filter(|r| r.enabled && !r.key.trim().is_empty())
                .map(|r| {
                    format!(
                        "{}={}",
                        encode_query_component(&interpolate(&r.key, scopes)),
                        encode_query_component(&interpolate(&r.value, scopes))
                    )
                })
                .collect();
            body = pairs.join("&").into_bytes();
            if !headers
                .iter()
                .any(|(k, _)| k.eq_ignore_ascii_case("content-type"))
            {
                headers.push((
                    "Content-Type".into(),
                    "application/x-www-form-urlencoded".into(),
                ));
            }
        }
        BodyKind::Multipart => {
            let (boundary, bytes) = multipart_body(&def.body.rows, scopes);
            body = bytes;
            headers.push((
                "Content-Type".into(),
                format!("multipart/form-data; boundary={boundary}"),
            ));
        }
        BodyKind::Binary { file_path } => {
            #[cfg(target_arch = "wasm32")]
            {
                let _ = file_path;
                return Err("Binary bodies require the desktop app".to_string());
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let path = interpolate(file_path, scopes);
                body =
                    std::fs::read(&path).map_err(|err| format!("could not read {path}: {err}"))?;
            }
        }
    }

    let headers = ehttp::Headers { headers };
    Ok(ehttp::Request {
        method: ehttp_method(def.method),
        url,
        body,
        headers,
        ..ehttp::Request::get("about:blank")
    })
}

/// Builds a [`ResponseSummary`] from a completed response.
pub fn summary_from_response(resp: &ehttp::Response, duration_ms: u64) -> ResponseSummary {
    ResponseSummary {
        status: resp.status,
        reason: if resp.status_text.is_empty() {
            "OK".to_string()
        } else {
            resp.status_text.clone()
        },
        size: resp.bytes.len() as u64,
        duration_ms,
        time: restrust_core::model::now_rfc3339(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use restrust_core::model::{Environment, KeyValue, RequestBody};
    use restrust_core::workspace_doc::ClientSettings;

    fn doc_with_env() -> WorkspaceDoc {
        let mut doc = WorkspaceDoc::new();
        doc.settings = ClientSettings {
            server_url: None,
            active_environment: Some("Staging".into()),
            theme: "dark".into(),
        };
        doc.environments.push(Environment {
            id: "e1".into(),
            name: "Staging".into(),
            values: vec![KeyValue::new("base_url", "https://api.example.com")],
        });
        doc.globals = vec![KeyValue::new("token", "glob-tok")];
        doc
    }

    #[test]
    fn builds_get_with_interpolated_url_and_query() {
        let doc = doc_with_env();
        let mut def = RequestDef::new(HttpMethod::Get, "list", "{{base_url}}/users");
        def.query = vec![
            KeyValue::new("page", "1"),
            KeyValue::new("disabled", "x"),
            KeyValue::new("empty", ""),
        ];
        def.query[1].enabled = false;
        let scopes = resolved_scopes(&doc, None);
        let req = build_request(&def, &AuthKind::None, &scopes).unwrap();
        assert_eq!(req.method.as_str(), "GET");
        assert_eq!(req.url, "https://api.example.com/users?page=1&empty=");
        assert!(req.body.is_empty());
    }

    #[test]
    fn missing_scheme_gets_https_prefix() {
        let def = RequestDef::new(HttpMethod::Get, "x", "api.example.com/p");
        let req = build_request(&def, &AuthKind::None, &[]).unwrap();
        assert_eq!(req.url, "https://api.example.com/p");
    }

    #[test]
    fn empty_url_is_actionable_error() {
        let def = RequestDef::new(HttpMethod::Get, "x", "  ");
        assert!(build_request(&def, &AuthKind::None, &[]).is_err());
    }

    #[test]
    fn bearer_and_basic_auth_headers() {
        let doc = doc_with_env();
        let scopes = resolved_scopes(&doc, None);
        let def = RequestDef::new(HttpMethod::Get, "x", "https://api.test");
        let req = build_request(
            &def,
            &AuthKind::Bearer {
                token: "{{token}}".into(),
            },
            &scopes,
        )
        .unwrap();
        assert_eq!(req.headers.get("authorization"), Some("Bearer glob-tok"));

        let req = build_request(
            &def,
            &AuthKind::Basic {
                username: "u".into(),
                password: "p".into(),
            },
            &[],
        )
        .unwrap();
        assert_eq!(req.headers.get("authorization"), Some("Basic dTpw"));
    }

    #[test]
    fn api_key_in_query_and_header() {
        let def = RequestDef::new(HttpMethod::Get, "x", "https://api.test");
        let req = build_request(
            &def,
            &AuthKind::ApiKey {
                key: "X-Api-Key".into(),
                value: "secret&".into(),
                add_to: restrust_core::model::ApiKeyIn::Query,
            },
            &[],
        )
        .unwrap();
        assert_eq!(req.url, "https://api.test?X-Api-Key=secret%26");

        let req = build_request(
            &def,
            &AuthKind::ApiKey {
                key: "X-Api-Key".into(),
                value: "secret".into(),
                add_to: restrust_core::model::ApiKeyIn::Header,
            },
            &[],
        )
        .unwrap();
        assert_eq!(req.headers.get("x-api-key"), Some("secret"));
    }

    #[test]
    fn raw_body_sets_content_type_and_interpolates() {
        let doc = doc_with_env();
        let scopes = resolved_scopes(&doc, None);
        let mut def = RequestDef::new(HttpMethod::Post, "create", "{{base_url}}/things");
        def.body = RequestBody {
            kind: BodyKind::Raw {
                mime: "application/json".into(),
            },
            raw: Some("{\"n\":\"{{token}}\"}".into()),
            rows: Vec::new(),
        };
        let req = build_request(&def, &AuthKind::None, &scopes).unwrap();
        assert_eq!(req.method.as_str(), "POST");
        assert_eq!(req.headers.get("content-type"), Some("application/json"));
        assert_eq!(req.body, b"{\"n\":\"glob-tok\"}".to_vec());
    }

    #[test]
    fn urlencoded_body_encodes_and_sets_header() {
        let mut def = RequestDef::new(HttpMethod::Post, "form", "https://api.test/f");
        def.body = RequestBody {
            kind: BodyKind::UrlEncoded,
            raw: None,
            rows: vec![KeyValue::new("a b", "1&2"), KeyValue::new("off", "x")],
        };
        def.body.rows[1].enabled = false;
        let req = build_request(&def, &AuthKind::None, &[]).unwrap();
        assert_eq!(
            req.headers.get("content-type"),
            Some("application/x-www-form-urlencoded")
        );
        assert_eq!(req.body, b"a%20b=1%262".to_vec());
    }

    #[test]
    fn multipart_body_shape() {
        let mut def = RequestDef::new(HttpMethod::Post, "multi", "https://api.test/m");
        def.body = RequestBody {
            kind: BodyKind::Multipart,
            raw: None,
            rows: vec![KeyValue::new("field", "value")],
        };
        let req = build_request(&def, &AuthKind::None, &[]).unwrap();
        let content_type = req.headers.get("content-type").unwrap().to_string();
        assert!(content_type.starts_with("multipart/form-data; boundary=restrust-"));
        let boundary = content_type.split("boundary=").nth(1).unwrap().to_string();
        let text = String::from_utf8(req.body.clone()).unwrap();
        assert!(text.starts_with(&format!("--{boundary}\r\n")));
        assert!(text.contains("Content-Disposition: form-data; name=\"field\"\r\n\r\nvalue\r\n"));
        assert!(text.ends_with(&format!("--{boundary}--\r\n")));
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn binary_body_reads_file() {
        let dir = std::env::temp_dir().join(format!("restrust-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("blob.bin");
        std::fs::write(&file, b"contents").unwrap();
        let mut def = RequestDef::new(HttpMethod::Put, "up", "https://api.test/u");
        def.body = RequestBody {
            kind: BodyKind::Binary {
                file_path: file.display().to_string(),
            },
            raw: None,
            rows: Vec::new(),
        };
        let req = build_request(&def, &AuthKind::None, &[]).unwrap();
        assert_eq!(req.body, b"contents".to_vec());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn query_components_percent_encode() {
        assert_eq!(encode_query_component("a b&c=d"), "a%20b%26c%3Dd");
        assert_eq!(encode_query_component("safe-._~"), "safe-._~");
    }

    #[test]
    fn base64_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn summary_from_response_fields() {
        let resp = ehttp::Response {
            url: "https://x".into(),
            ok: true,
            status: 201,
            status_text: "Created".into(),
            headers: Default::default(),
            bytes: vec![0; 512],
        };
        let summary = summary_from_response(&resp, 42);
        assert_eq!(summary.status, 201);
        assert_eq!(summary.reason, "Created");
        assert_eq!(summary.size, 512);
        assert_eq!(summary.duration_ms, 42);
        assert!(summary.time.ends_with('Z'));
    }

    #[test]
    fn scopes_order_env_then_globals() {
        let doc = doc_with_env();
        let scopes = resolved_scopes(&doc, None);
        assert_eq!(
            interpolate("{{base_url}}", &scopes),
            "https://api.example.com"
        );
        assert_eq!(interpolate("{{token}}", &scopes), "glob-tok");
    }
}
