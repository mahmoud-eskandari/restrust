//! Smoke tests for snippet generation.

use restrust_core::model::{
    AuthConfig, AuthKind, BodyKind, HttpMethod, KeyValue, RequestBody, RequestDef,
};
use restrust_interchange::snippet::{generate, Target};

fn sample_bearer_json() -> RequestDef {
    let mut r = RequestDef::new(
        HttpMethod::Post,
        "create",
        "https://api.example.test/things",
    );
    r.query = vec![KeyValue::new("dry", "true")];
    r.headers = vec![KeyValue::new("Accept", "application/json")];
    r.body = RequestBody {
        kind: BodyKind::Raw {
            mime: "application/json".to_string(),
        },
        raw: Some("{\"a\": 1}".to_string()),
        rows: Vec::new(),
    };
    r.auth = Some(AuthConfig {
        kind: AuthKind::Bearer {
            token: "tok123".to_string(),
        },
    });
    r
}

fn sample_basic() -> RequestDef {
    let mut r = RequestDef::new(HttpMethod::Get, "list", "https://api.example.test/items");
    r.auth = Some(AuthConfig {
        kind: AuthKind::Basic {
            username: "user".to_string(),
            password: "pass".to_string(),
        },
    });
    r
}

fn sample_urlencoded() -> RequestDef {
    let mut r = RequestDef::new(HttpMethod::Post, "form", "https://api.example.test/f");
    r.body = RequestBody {
        kind: BodyKind::UrlEncoded,
        raw: None,
        rows: vec![KeyValue::new("a", "1")],
    };
    r
}

#[test]
fn curl_target_reuses_curl_export() {
    let s = generate(&sample_bearer_json(), Target::Curl);
    assert!(s.starts_with("curl "), "{s}");
    assert!(s.contains("?dry=true"), "{s}");
    assert!(s.contains("-H 'Authorization: Bearer tok123'"), "{s}");
    assert!(s.contains("--data-raw '{\"a\": 1}'"), "{s}");
}

#[test]
fn rust_reqwest_snippet() {
    let s = generate(&sample_bearer_json(), Target::RustReqwest);
    assert!(
        s.contains(".post(\"https://api.example.test/things?dry=true\")"),
        "{s}"
    );
    assert!(
        s.contains(".header(\"Accept\", \"application/json\")"),
        "{s}"
    );
    assert!(s.contains(".bearer_auth(\"tok123\")"), "{s}");
    assert!(s.contains(".body(r#\"{\"a\": 1}\"#)"), "{s}");
    assert!(s.contains(".send()\n    .await?;"), "{s}");

    // OPTIONS has no reqwest convenience builder
    let mut r = sample_bearer_json();
    r.method = HttpMethod::Options;
    let s = generate(&r, Target::RustReqwest);
    assert!(s.contains("reqwest::Method::OPTIONS"), "{s}");

    let s = generate(&sample_basic(), Target::RustReqwest);
    assert!(s.contains(".basic_auth(\"user\", Some(\"pass\"))"), "{s}");

    let s = generate(&sample_urlencoded(), Target::RustReqwest);
    assert!(
        s.contains(".form(&[\n        (\"a\", \"1\"),\n    ])"),
        "{s}"
    );
}

#[test]
fn python_requests_snippet() {
    let s = generate(&sample_bearer_json(), Target::PythonRequests);
    assert!(s.contains("import requests"), "{s}");
    assert!(s.contains("requests.post(url"), "{s}");
    assert!(s.contains("'Authorization': 'Bearer tok123'"), "{s}");
    assert!(s.contains("data='{\"a\": 1}'"), "{s}");
    assert!(s.contains("print(response.status_code)"), "{s}");

    let s = generate(&sample_basic(), Target::PythonRequests);
    assert!(s.contains("auth = ('user', 'pass')"), "{s}");
    assert!(s.contains("auth=auth"), "{s}");

    let s = generate(&sample_urlencoded(), Target::PythonRequests);
    assert!(s.contains("data={'a': '1'}"), "{s}");
}

#[test]
fn js_fetch_snippet() {
    let s = generate(&sample_bearer_json(), Target::JsFetch);
    assert!(
        s.contains("await fetch('https://api.example.test/things?dry=true'"),
        "{s}"
    );
    assert!(s.contains("method: \"POST\","), "{s}");
    assert!(s.contains("'Authorization': 'Bearer tok123'"), "{s}");
    assert!(s.contains("body: '{\"a\": 1}',"), "{s}");

    let s = generate(&sample_basic(), Target::JsFetch);
    assert!(s.contains("btoa(\"user:pass\")"), "{s}");
}

#[test]
fn go_net_http_snippet() {
    let s = generate(&sample_bearer_json(), Target::GoNetHttp);
    assert!(s.contains("package main"), "{s}");
    assert!(s.contains("http.MethodPost"), "{s}");
    assert!(s.contains("strings.NewReader(`{\"a\": 1}`)"), "{s}");
    assert!(
        s.contains("req.Header.Add(\"Authorization\", \"Bearer tok123\")"),
        "{s}"
    );
    assert!(s.contains("http.DefaultClient.Do(req)"), "{s}");

    let s = generate(&sample_basic(), Target::GoNetHttp);
    assert!(s.contains("req.SetBasicAuth(\"user\", \"pass\")"), "{s}");

    let s = generate(&sample_urlencoded(), Target::GoNetHttp);
    assert!(s.contains("strings.NewReader(`a=1`)"), "{s}");
}

#[test]
fn multipart_and_binary_bodies() {
    let mut r = RequestDef::new(HttpMethod::Post, "multi", "https://api.example.test/m");
    r.body = RequestBody {
        kind: BodyKind::Multipart,
        raw: None,
        rows: vec![KeyValue::new("field", "value")],
    };

    let s = generate(&r, Target::RustReqwest);
    assert!(s.contains("reqwest::multipart::Form::new()"), "{s}");
    assert!(s.contains(".text(\"field\", \"value\")"), "{s}");

    let s = generate(&r, Target::PythonRequests);
    assert!(s.contains("files={'field': (None, 'value')}"), "{s}");

    let s = generate(&r, Target::JsFetch);
    assert!(s.contains("new FormData()"), "{s}");
    assert!(s.contains("form.append('field', 'value');"), "{s}");

    let s = generate(&r, Target::GoNetHttp);
    assert!(s.contains("multipart form data is not rendered"), "{s}");

    let mut r = RequestDef::new(HttpMethod::Post, "bin", "https://api.example.test/b");
    r.body = RequestBody {
        kind: BodyKind::Binary {
            file_path: "/tmp/blob.bin".to_string(),
        },
        raw: None,
        rows: Vec::new(),
    };
    assert!(generate(&r, Target::Curl).contains("--data-binary"));
    assert!(generate(&r, Target::RustReqwest).contains("std::fs::read"));
    assert!(generate(&r, Target::GoNetHttp).contains("os.ReadFile"));
    assert!(generate(&r, Target::PythonRequests).contains("data=open('/tmp/blob.bin', 'rb')"));
}

#[test]
fn bodyless_get_has_no_body_lines() {
    let r = RequestDef::new(HttpMethod::Get, "plain", "https://api.example.test/ping");
    for t in [
        Target::Curl,
        Target::RustReqwest,
        Target::PythonRequests,
        Target::JsFetch,
        Target::GoNetHttp,
    ] {
        let s = generate(&r, t);
        assert!(!s.contains("data="), "{t:?}: {s}");
        assert!(!s.contains("--data"), "{t:?}: {s}");
        assert!(!s.contains("body:"), "{t:?}: {s}");
    }
}
