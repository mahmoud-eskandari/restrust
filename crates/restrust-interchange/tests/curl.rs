//! Integration tests for cURL import/export.

use restrust_core::model::{
    ApiKeyIn, AuthConfig, AuthKind, BodyKind, HttpMethod, KeyValue, RequestBody, RequestDef,
};
use restrust_interchange::curl;

#[test]
fn import_full_command() {
    let cmd = concat!(
        "curl -X PUT 'https://api.example.test/things/1?dry=true' \\\n",
        "  -H 'Content-Type: application/json' \\\n",
        "  -H \"X-Note: it's \\\"fine\\\"\" \\\n",
        "  --data-raw '{\"name\": \"thing one\"}' \\\n",
        "  -u 'user:pass' \\\n",
        "  -b 'session=abc123' \\\n",
        "  --compressed -k"
    );
    let r = curl::import_curl(cmd).unwrap();
    assert_eq!(r.method, HttpMethod::Put);
    assert_eq!(r.url, "https://api.example.test/things/1?dry=true");
    assert_eq!(r.headers.len(), 3);
    assert_eq!(r.headers[0].key, "Content-Type");
    assert_eq!(r.headers[0].value, "application/json");
    assert_eq!(r.headers[1].key, "X-Note");
    assert_eq!(r.headers[1].value, "it's \"fine\"");
    assert_eq!(r.headers[2].key, "Cookie");
    assert_eq!(r.headers[2].value, "session=abc123");
    assert_eq!(
        r.body.kind,
        BodyKind::Raw {
            mime: "application/json".to_string()
        }
    );
    assert_eq!(r.body.raw.as_deref(), Some("{\"name\": \"thing one\"}"));
    assert_eq!(
        r.auth.map(|a| a.kind),
        Some(AuthKind::Basic {
            username: "user".to_string(),
            password: "pass".to_string(),
        })
    );
    // unmappable flags are noted in the description
    let desc = r.description.unwrap();
    assert!(desc.contains("--compressed"), "{desc}");
    assert!(desc.contains("-k"), "{desc}");
}

#[test]
fn import_defaults_and_data_implies_post() {
    let r = curl::import_curl("curl https://x.test/a").unwrap();
    assert_eq!(r.method, HttpMethod::Get);
    assert_eq!(r.body.kind, BodyKind::None);
    assert_eq!(r.description, None);

    let r = curl::import_curl("curl -d 'a=1' -d 'b=2' https://x.test/form").unwrap();
    assert_eq!(r.method, HttpMethod::Post);
    assert_eq!(
        r.body.kind,
        BodyKind::Raw {
            mime: "application/x-www-form-urlencoded".to_string()
        }
    );
    assert_eq!(r.body.raw.as_deref(), Some("a=1&b=2"));

    // an explicit Content-Type header wins over the default
    let r = curl::import_curl("curl -H 'Content-Type: text/plain' --data-raw hi https://x.test/t")
        .unwrap();
    assert_eq!(
        r.body.kind,
        BodyKind::Raw {
            mime: "text/plain".to_string()
        }
    );

    // --json implies application/json
    let r = curl::import_curl("curl --json '{\"a\":1}' https://x.test/j").unwrap();
    assert_eq!(
        r.body.kind,
        BodyKind::Raw {
            mime: "application/json".to_string()
        }
    );
}

#[test]
fn import_get_flag_moves_data_to_query() {
    let r = curl::import_curl("curl -G --data 'a=1' --data 'b=2' https://x.test/search").unwrap();
    assert_eq!(r.method, HttpMethod::Get);
    assert_eq!(r.url, "https://x.test/search?a=1&b=2");
    assert_eq!(r.body.kind, BodyKind::None);
}

#[test]
fn import_form_and_user_agent() {
    let r = curl::import_curl(
        "curl -F 'note=hi' -F 'file=@a.png' -A 'restrust/1.0' https://x.test/upload",
    )
    .unwrap();
    assert_eq!(r.method, HttpMethod::Post);
    assert_eq!(r.body.kind, BodyKind::Multipart);
    assert_eq!(r.body.rows.len(), 2);
    assert_eq!(r.body.rows[0].key, "note");
    assert_eq!(r.body.rows[1].value, "a.png");
    assert!(r
        .headers
        .iter()
        .any(|h| h.key == "User-Agent" && h.value == "restrust/1.0"));
}

#[test]
fn import_errors() {
    assert!(curl::import_curl("curl -H 'X: y'").is_err(), "no URL");
    assert!(curl::import_curl("curl 'https://x.test/unterminated").is_err());
    assert!(curl::import_curl("curl -X BREW https://x.test/coffee").is_err());
    assert!(curl::import_curl("").is_err());
    // missing value for a value-taking flag
    assert!(curl::import_curl("curl -H").is_err());
}

#[test]
fn export_then_import_is_equivalent() {
    let mut r = RequestDef::new(
        HttpMethod::Post,
        "create",
        "https://api.example.test/things",
    );
    r.query = vec![
        KeyValue::new("dry", "true"),
        KeyValue {
            key: "off".to_string(),
            value: "1".to_string(),
            enabled: false,
            description: None,
        },
    ];
    r.headers = vec![
        KeyValue::new("Content-Type", "application/json"),
        KeyValue {
            key: "X-Off".to_string(),
            value: "1".to_string(),
            enabled: false,
            description: None,
        },
    ];
    r.body = RequestBody {
        kind: BodyKind::Raw {
            mime: "application/json".to_string(),
        },
        raw: Some("{\"a\": 1}".to_string()),
        rows: Vec::new(),
    };
    r.auth = Some(AuthConfig {
        kind: AuthKind::Basic {
            username: "user".to_string(),
            password: "pass".to_string(),
        },
    });

    let cmd = curl::export_curl(&r);
    assert!(cmd.contains("-X POST"), "{cmd}");
    assert!(
        cmd.contains("'https://api.example.test/things?dry=true'"),
        "{cmd}"
    );
    assert!(cmd.contains("-H 'Content-Type: application/json'"), "{cmd}");
    assert!(cmd.contains("--data-raw '{\"a\": 1}'"), "{cmd}");
    assert!(cmd.contains("--user 'user:pass'"), "{cmd}");
    assert!(!cmd.contains("off=1") && !cmd.contains("X-Off"), "{cmd}");

    let back = curl::import_curl(&cmd).unwrap();
    assert_eq!(back.method, r.method);
    assert_eq!(back.url, "https://api.example.test/things?dry=true");
    assert_eq!(back.body.raw, r.body.raw);
    assert_eq!(back.body.kind, r.body.kind);
    assert_eq!(back.auth.map(|a| a.kind), r.auth.map(|a| a.kind));
    assert!(back.headers.iter().any(|h| h.key == "Content-Type"));
    assert!(!back.headers.iter().any(|h| h.key == "X-Off"));
}

#[test]
fn export_bodies_and_auth_kinds() {
    // urlencoded joins rows and sets the content type
    let mut r = RequestDef::new(HttpMethod::Put, "form", "https://x.test/f");
    r.body = RequestBody {
        kind: BodyKind::UrlEncoded,
        raw: None,
        rows: vec![KeyValue::new("a", "1"), KeyValue::new("b", "2")],
    };
    let cmd = curl::export_curl(&r);
    assert!(cmd.contains("--data-raw 'a=1&b=2'"), "{cmd}");
    assert!(
        cmd.contains("-H 'Content-Type: application/x-www-form-urlencoded'"),
        "{cmd}"
    );

    // multipart rows become -F
    let mut r = RequestDef::new(HttpMethod::Post, "multi", "https://x.test/m");
    r.body = RequestBody {
        kind: BodyKind::Multipart,
        raw: None,
        rows: vec![KeyValue::new("field", "value")],
    };
    assert!(curl::export_curl(&r).contains("-F 'field=value'"));

    // binary bodies reference the file
    let mut r = RequestDef::new(HttpMethod::Post, "bin", "https://x.test/b");
    r.body = RequestBody {
        kind: BodyKind::Binary {
            file_path: "/tmp/blob.bin".to_string(),
        },
        raw: None,
        rows: Vec::new(),
    };
    assert!(curl::export_curl(&r).contains("--data-binary '@/tmp/blob.bin'"));

    // bearer auth becomes an Authorization header; api-key-in-query joins the URL
    let mut r = RequestDef::new(HttpMethod::Get, "authd", "https://x.test/a?x=1");
    r.auth = Some(AuthConfig {
        kind: AuthKind::Bearer {
            token: "tok".to_string(),
        },
    });
    let cmd = curl::export_curl(&r);
    assert!(cmd.contains("-H 'Authorization: Bearer tok'"), "{cmd}");

    let mut r = RequestDef::new(HttpMethod::Get, "keyed", "https://x.test/a");
    r.auth = Some(AuthConfig {
        kind: AuthKind::ApiKey {
            key: "api_key".to_string(),
            value: "v".to_string(),
            add_to: ApiKeyIn::Query,
        },
    });
    assert!(curl::export_curl(&r).contains("'https://x.test/a?api_key=v'"));

    // single quotes in values are escaped
    let mut r = RequestDef::new(HttpMethod::Get, "q", "https://x.test/a");
    r.headers = vec![KeyValue::new("X-It", "it's fine")];
    assert!(curl::export_curl(&r).contains("it'\\''s fine"));
}
