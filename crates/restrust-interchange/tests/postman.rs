//! Integration tests for Postman collection/environment import and export.

use restrust_core::model::{
    ApiKeyIn, AuthConfig, AuthKind, BodyKind, Collection, CollectionItem, EventListen, Folder,
    HttpMethod, KeyValue, RequestBody, RequestDef,
};
use restrust_interchange::postman;

const V2_1_FIXTURE: &str = include_str!("fixtures/postman_v2_1.json");
const V3_FIXTURE: &str = include_str!("fixtures/postman_v3.json");
const ENV_FIXTURE: &str = include_str!("fixtures/postman_environment.json");

/// Finds a request anywhere in the tree by name.
fn request<'a>(c: &'a Collection, name: &str) -> &'a restrust_core::model::RequestDef {
    fn walk<'a>(
        items: &'a [CollectionItem],
        name: &str,
    ) -> Option<&'a restrust_core::model::RequestDef> {
        for item in items {
            match item {
                CollectionItem::Request(r) if r.name == name => return Some(r),
                CollectionItem::Folder(f) => {
                    if let Some(found) = walk(&f.items, name) {
                        return Some(found);
                    }
                }
                _ => {}
            }
        }
        None
    }
    walk(&c.items, name).unwrap_or_else(|| panic!("request '{name}' not found"))
}

/// Finds a folder anywhere in the tree by name.
fn folder<'a>(c: &'a Collection, name: &str) -> &'a restrust_core::model::Folder {
    fn walk<'a>(
        items: &'a [CollectionItem],
        name: &str,
    ) -> Option<&'a restrust_core::model::Folder> {
        for item in items {
            if let CollectionItem::Folder(f) = item {
                if f.name == name {
                    return Some(f);
                }
                if let Some(found) = walk(&f.items, name) {
                    return Some(found);
                }
            }
        }
        None
    }
    walk(&c.items, name).unwrap_or_else(|| panic!("folder '{name}' not found"))
}

fn zero_ids(c: &mut Collection) {
    fn walk(items: &mut [CollectionItem]) {
        for item in items {
            match item {
                CollectionItem::Request(r) => r.id = String::new(),
                CollectionItem::Folder(f) => {
                    f.id = String::new();
                    walk(&mut f.items);
                }
            }
        }
    }
    c.id = String::new();
    walk(&mut c.items);
}

#[test]
fn import_v2_1_fixture_full_tree() {
    let imported = postman::import_v2_1(V2_1_FIXTURE).unwrap();
    assert_eq!(imported.collections.len(), 1);
    let c = &imported.collections[0];

    // collection-level metadata
    assert_eq!(c.id, "11111111-1111-1111-1111-111111111111");
    assert_eq!(c.name, "Demo API");
    assert_eq!(
        c.description.as_deref(),
        Some("Fixture collection exercising the v2.1 import surface")
    );
    assert_eq!(c.variables.len(), 2);
    assert_eq!(c.variables[0].key, "base_url");
    assert_eq!(c.variables[0].value, "https://api.demo.test");
    assert!(c.variables[0].enabled);
    assert_eq!(c.variables[1].key, "api_key");
    assert_eq!(c.auth, None);

    // collection event: exec lines joined into one script
    assert_eq!(c.events.len(), 1);
    assert_eq!(c.events[0].listen, EventListen::PreRequest);
    assert_eq!(
        c.events[0].script,
        "console.log('setting up');\npm.environment.get('api_key');"
    );

    // tree: folder v1 -> { request login, folder admin -> { create user, audit log } }
    assert_eq!(c.items.len(), 1);
    let v1 = folder(c, "v1");
    assert_eq!(v1.description.as_deref(), Some("versioned endpoints"));
    assert_eq!(v1.items.len(), 2);
    let admin = folder(c, "admin");
    assert_eq!(
        admin.description.as_deref(),
        Some("administrative endpoints")
    );
    assert_eq!(admin.items.len(), 2);

    // login: urlencoded body with a disabled row
    let login = request(c, "login");
    assert_eq!(login.method, HttpMethod::Post);
    assert_eq!(login.url, "{{base_url}}/login");
    assert_eq!(
        login.description.as_deref(),
        Some("Exchange credentials for a token.")
    );
    assert_eq!(login.headers.len(), 1);
    assert_eq!(login.headers[0].key, "Accept");
    assert_eq!(login.headers[0].value, "application/json");
    assert_eq!(
        login.headers[0].description.as_deref(),
        Some("always accept JSON")
    );
    assert!(login.query.is_empty());
    assert_eq!(login.body.kind, BodyKind::UrlEncoded);
    assert_eq!(login.body.rows.len(), 3);
    assert_eq!(login.body.rows[0].key, "username");
    assert_eq!(login.body.rows[0].value, "admin");
    assert!(login.body.rows[0].enabled);
    assert_eq!(login.body.rows[2].key, "otp");
    assert!(!login.body.rows[2].enabled);

    // create user: raw json body, query rows incl. disabled, bearer auth
    let create = request(c, "create user");
    assert_eq!(create.method, HttpMethod::Post);
    assert_eq!(create.url, "{{base_url}}/users");
    assert_eq!(
        create.body.kind,
        BodyKind::Raw {
            mime: "application/json".to_string()
        }
    );
    assert_eq!(
        create.body.raw.as_deref(),
        Some("{\n  \"name\": \"ada\",\n  \"role\": \"admin\"\n}")
    );
    assert_eq!(create.query.len(), 3);
    assert_eq!(create.query[0].key, "page");
    assert!(create.query[0].enabled);
    assert!(!create.query[2].enabled);
    assert_eq!(create.headers.len(), 2);
    assert!(!create.headers[1].enabled);
    assert_eq!(
        create.auth.as_ref().map(|a| a.kind.clone()),
        Some(AuthKind::Bearer {
            token: "{{api_key}}".to_string()
        })
    );

    // audit log: plain string URL kept verbatim (query string included)
    let audit = request(c, "audit log");
    assert_eq!(audit.method, HttpMethod::Get);
    assert_eq!(audit.url, "{{base_url}}/audit?since=2026-01-01");

    // report
    assert!(imported
        .report
        .preserved
        .iter()
        .any(|e| e.contains("prerequest")));
    assert!(imported
        .report
        .unsupported
        .iter()
        .any(|e| e.contains("protocolProfileBehavior")));
    assert!(
        imported.report.lossy.is_empty(),
        "{:?}",
        imported.report.lossy
    );
}

#[test]
fn detect_and_import_routes_v2_1_identically() {
    let detected = postman::detect_and_import(V2_1_FIXTURE).unwrap();
    let direct = postman::import_v2_1(V2_1_FIXTURE).unwrap();
    assert_eq!(detected.report, direct.report);
    let mut a = detected.collections[0].clone();
    let mut b = direct.collections[0].clone();
    zero_ids(&mut a);
    zero_ids(&mut b);
    assert_eq!(a, b);
}

#[test]
fn export_v2_1_round_trips() {
    let first = postman::import_v2_1(V2_1_FIXTURE).unwrap();
    let exported = postman::export_v2_1(&first.collections[0]).unwrap();

    // the export claims v2.1 and re-imports
    assert!(exported.contains("/v2.1.0/collection.json"));
    let second = postman::import_v2_1(&exported).unwrap();

    let mut a = first.collections[0].clone();
    let mut b = second.collections[0].clone();
    zero_ids(&mut a);
    zero_ids(&mut b);
    assert_eq!(a, b);
}

#[test]
fn export_v2_1_round_trips_every_auth_and_body_kind() {
    let mut c = Collection::new("Kitchen sink");
    c.auth = Some(AuthConfig {
        kind: AuthKind::ApiKey {
            key: "X-Key".to_string(),
            value: "v".to_string(),
            add_to: ApiKeyIn::Query,
        },
    });
    c.variables = vec![
        KeyValue::new("base_url", "https://api.example.test"),
        KeyValue {
            key: "old".to_string(),
            value: "x".to_string(),
            enabled: false,
            description: None,
        },
    ];
    let mut full = RequestDef::new(
        HttpMethod::Post,
        "full",
        "https://api.example.test:8443/things",
    );
    full.query = vec![KeyValue::new("page", "2")];
    full.headers = vec![KeyValue::new("Accept", "application/json")];
    full.body = RequestBody {
        kind: BodyKind::Multipart,
        raw: None,
        rows: vec![KeyValue::new("field", "value")],
    };
    full.auth = Some(AuthConfig {
        kind: AuthKind::None,
    });
    let mut bin = RequestDef::new(HttpMethod::Put, "upload", "{{base_url}}/upload");
    bin.body = RequestBody {
        kind: BodyKind::Binary {
            file_path: "/tmp/blob.bin".to_string(),
        },
        raw: None,
        rows: Vec::new(),
    };
    let mut basic = RequestDef::new(HttpMethod::Get, "whoami", "{{base_url}}/me");
    basic.auth = Some(AuthConfig {
        kind: AuthKind::Basic {
            username: "u".to_string(),
            password: "p".to_string(),
        },
    });
    c.items = vec![
        CollectionItem::Request(full),
        CollectionItem::Folder(Folder {
            id: "f".to_string(),
            name: "misc".to_string(),
            description: None,
            items: vec![CollectionItem::Request(bin)],
            auth: None,
        }),
        CollectionItem::Request(basic),
    ];

    let exported = postman::export_v2_1(&c).unwrap();
    let imported = postman::import_v2_1(&exported).unwrap();
    let mut b = imported.collections[0].clone();
    zero_ids(&mut b);
    zero_ids(&mut c);
    assert_eq!(c, b);
}

#[test]
fn import_v3_fixture() {
    let imported = postman::import_v3(V3_FIXTURE).unwrap();
    assert_eq!(imported.collections.len(), 1);
    let c = &imported.collections[0];
    assert_eq!(c.name, "V3 Demo");
    assert_eq!(c.description.as_deref(), Some("A small v3 collection"));
    assert_eq!(c.variables.len(), 1);
    assert_eq!(c.variables[0].key, "base_url");
    assert_eq!(c.variables[0].value, "https://api.demo.test");

    let health = request(c, "Health check");
    assert_eq!(health.method, HttpMethod::Get);
    assert_eq!(health.url, "{{base_url}}/health");
    assert_eq!(health.headers.len(), 1);
    assert_eq!(health.headers[0].key, "Accept");

    let things = folder(c, "Things");
    assert_eq!(things.description.as_deref(), Some("thing operations"));
    assert_eq!(things.items.len(), 1);
    let create = request(c, "Create thing");
    assert_eq!(create.method, HttpMethod::Post);
    assert_eq!(create.url, "{{base_url}}/things");
    assert_eq!(
        create.body.kind,
        BodyKind::Raw {
            mime: "application/json".to_string()
        }
    );
    assert_eq!(create.body.raw.as_deref(), Some("{\"name\":\"x\"}"));
    assert_eq!(
        create.auth.as_ref().map(|a| a.kind.clone()),
        Some(AuthKind::Bearer {
            token: "abc123".to_string()
        })
    );

    assert!(imported
        .report
        .lossy
        .iter()
        .any(|e| e.contains("version tag '1.0.0'")));
    // v3 fixture is also reachable through detection
    let detected = postman::detect_and_import(V3_FIXTURE).unwrap();
    assert_eq!(detected.collections.len(), 1);
    assert_eq!(detected.collections[0].name, "V3 Demo");
}

#[test]
fn import_environment_and_detection() {
    let env = postman::import_environment(ENV_FIXTURE).unwrap();
    assert_eq!(env.name, "Demo Dev");
    assert_eq!(env.values.len(), 2);
    assert_eq!(env.values[0].key, "base_url");
    assert!(env.values[0].enabled);
    assert!(!env.values[1].enabled);

    let imported = postman::detect_and_import(ENV_FIXTURE).unwrap();
    assert!(imported.collections.is_empty());
    assert_eq!(imported.environments.len(), 1);
    let got = &imported.environments[0];
    assert_eq!(got.name, "Demo Dev");
    assert_eq!(got.values, env.values);
    assert!(imported
        .report
        .preserved
        .iter()
        .any(|e| e.contains("environment 'Demo Dev'")));
}

#[test]
fn formdata_files_path_variables_and_unsupported_modes_are_reported() {
    let json = r##"{
      "info": {
        "name": "Edge",
        "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
      },
      "item": [
        {
          "name": "upload",
          "event": [ { "listen": "test", "script": { "exec": ["pm.test('x');"] } } ],
          "request": {
            "method": "POST",
            "url": {
              "raw": "https://api.test/files/:id",
              "protocol": "https",
              "host": ["api", "test"],
              "path": ["files", ":id"],
              "variable": [ { "key": "id", "value": "42" } ]
            },
            "body": {
              "mode": "formdata",
              "formdata": [
                { "key": "note", "value": "hi", "type": "text" },
                { "key": "attachment", "src": "a.png", "type": "file" }
              ]
            }
          }
        },
        {
          "name": "legacy",
          "request": {
            "method": "POST",
            "url": "https://api.test/gql",
            "body": { "mode": "graphql", "query": "{}" },
            "auth": { "type": "oauth2", "oauth2": [] }
          }
        }
      ]
    }"##;
    let imported = postman::import_v2_1(json).unwrap();
    let c = &imported.collections[0];

    let upload = request(c, "upload");
    assert_eq!(upload.url, "https://api.test/files/42");
    assert_eq!(upload.body.kind, BodyKind::Multipart);
    assert_eq!(upload.body.rows.len(), 2);
    assert_eq!(upload.body.rows[1].key, "attachment");
    assert_eq!(upload.body.rows[1].value, "a.png");

    assert!(imported
        .report
        .lossy
        .iter()
        .any(|e| e == "request 'upload': form-data file 'a.png' preserved as name only"));
    assert!(imported
        .report
        .lossy
        .iter()
        .any(|e| e.contains("path variable 'id' inlined as '42'")));
    assert!(imported
        .report
        .unsupported
        .iter()
        .any(|e| e.contains("body mode 'graphql'")));
    assert!(imported
        .report
        .unsupported
        .iter()
        .any(|e| e.contains("auth type 'oauth2'")));
    assert!(imported
        .report
        .unsupported
        .iter()
        .any(|e| e.contains("script events are not modeled on requests")));

    let legacy = request(c, "legacy");
    assert_eq!(legacy.body.kind, BodyKind::None);
    assert_eq!(legacy.auth, None);
}

#[test]
fn detect_rejects_old_and_unrecognized_documents() {
    let v2 = r#"{"info": {"name": "x", "schema": "https://schema.getpostman.com/json/collection/v2.0.0/collection.json"}, "item": []}"#;
    let err = postman::detect_and_import(v2).unwrap_err();
    assert!(err.to_string().contains("v2.0"), "{err}");
    assert!(matches!(
        err,
        restrust_interchange::InterchangeError::UnsupportedFormat(_)
    ));

    assert!(postman::detect_and_import("not json at all").is_err());
    assert!(postman::detect_and_import("{}").is_err());
}

#[test]
fn export_v2_1_shapes_a_valid_postman_document() {
    let imported = postman::import_v2_1(V2_1_FIXTURE).unwrap();
    let exported = postman::export_v2_1(&imported.collections[0]).unwrap();
    let v: serde_json::Value = serde_json::from_str(&exported).unwrap();
    assert_eq!(
        v["info"]["schema"],
        "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"
    );
    assert_eq!(
        v["info"]["_postman_id"],
        "11111111-1111-1111-1111-111111111111"
    );
    assert_eq!(v["info"]["name"], "Demo API");
    assert_eq!(v["item"].as_array().map(Vec::len), Some(1));
    assert_eq!(v["variable"].as_array().map(Vec::len), Some(2));
    assert_eq!(v["event"][0]["listen"], "prerequest");
    assert_eq!(
        v["event"][0]["script"]["exec"],
        serde_json::json!([
            "console.log('setting up');",
            "pm.environment.get('api_key');"
        ])
    );
    // nested request carries its bearer auth
    let auth = &v["item"][0]["item"][1]["item"][0]["request"]["auth"];
    assert_eq!(auth["type"], "bearer");
    assert_eq!(auth["bearer"][0]["value"], "{{api_key}}");
}
