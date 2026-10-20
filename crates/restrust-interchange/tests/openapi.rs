//! Integration tests for OpenAPI 3.0/3.1 import and 3.0 export.

use restrust_core::model::{AuthKind, BodyKind, CollectionItem, HttpMethod};
use restrust_interchange::openapi;

const OPENAPI_3_0: &str = include_str!("fixtures/openapi_3_0.yaml");
const OPENAPI_3_1: &str = include_str!("fixtures/openapi_3_1.yaml");

fn request<'a>(
    c: &'a restrust_core::model::Collection,
    name: &str,
) -> &'a restrust_core::model::RequestDef {
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

#[test]
fn import_3_0_fixture() {
    let imported = openapi::detect_and_import(OPENAPI_3_0).unwrap();
    assert_eq!(imported.collections.len(), 1);
    let c = &imported.collections[0];
    assert_eq!(c.name, "Petstore Demo");
    assert_eq!(
        c.description.as_deref(),
        Some("A small petstore-like API for tests")
    );

    // untagged requests come first, tag folders after
    assert_eq!(c.items.len(), 2);
    assert!(matches!(&c.items[0], CollectionItem::Request(_)));
    assert!(matches!(&c.items[1], CollectionItem::Folder(f) if f.name == "pets"));
    let pets_folder = match &c.items[1] {
        CollectionItem::Folder(f) => f,
        _ => unreachable!(),
    };
    assert_eq!(pets_folder.items.len(), 2);

    // listPets: operationId name, server+path URL, merged path/op params
    let list = request(c, "listPets");
    assert_eq!(list.method, HttpMethod::Get);
    assert_eq!(list.url, "https://petstore.example.test/api/v1/pets");
    assert_eq!(list.query.len(), 2);
    let limit = list.query.iter().find(|q| q.key == "limit").unwrap();
    assert_eq!(limit.value, "0");
    assert!(!limit.enabled, "optional param is disabled");
    assert_eq!(
        limit.description.as_deref(),
        Some("how many items to return")
    );
    let page = list.query.iter().find(|q| q.key == "page").unwrap();
    assert!(page.enabled, "required param is enabled");
    assert_eq!(
        list.auth.as_ref().map(|a| a.kind.clone()),
        Some(AuthKind::Bearer {
            token: "{{bearerToken}}".to_string()
        })
    );
    assert!(list
        .description
        .as_deref()
        .unwrap()
        .contains("List all pets"));

    // createPet: JSON body sampled from the schema ($ref, example, enum)
    let create = request(c, "createPet");
    assert_eq!(create.method, HttpMethod::Post);
    assert_eq!(
        create.body.kind,
        BodyKind::Raw {
            mime: "application/json".to_string()
        }
    );
    let sample: serde_json::Value =
        serde_json::from_str(create.body.raw.as_deref().unwrap()).unwrap();
    assert_eq!(sample["name"], "doggie");
    assert_eq!(sample["id"], 0);
    assert_eq!(sample["status"], "available");
    assert_eq!(sample["tag"]["label"], "string");
    assert_eq!(
        create.auth.as_ref().map(|a| a.kind.clone()),
        Some(AuthKind::ApiKey {
            key: "X-Api-Key".to_string(),
            value: "{{apiKey}}".to_string(),
            add_to: restrust_core::model::ApiKeyIn::Header,
        })
    );

    // operation without operationId: summary becomes the name
    let info = request(c, "Info for a specific pet");
    assert_eq!(info.method, HttpMethod::Get);
    assert_eq!(
        info.url,
        "https://petstore.example.test/api/v1/pets/{petId}"
    );
    assert!(info.description.as_deref().unwrap().contains("petId"));
    assert_eq!(
        info.auth.as_ref().map(|a| a.kind.clone()),
        Some(AuthKind::Bearer {
            token: "{{bearerToken}}".to_string()
        })
    );

    // report entries
    assert!(imported
        .report
        .lossy
        .iter()
        .any(|e| e.contains("security scheme 'petstore_auth'")));
    assert!(imported
        .report
        .lossy
        .iter()
        .any(|e| e.contains("security scheme 'api_key'")));
    assert!(imported
        .report
        .unsupported
        .iter()
        .any(|e| e.contains("'deprecated' flag")));
    assert!(imported
        .report
        .unsupported
        .iter()
        .any(|e| e.contains("3 response definition(s)")));
}

#[test]
fn import_3_1_minimal_fixture() {
    let imported = openapi::detect_and_import(OPENAPI_3_1).unwrap();
    let c = &imported.collections[0];
    assert_eq!(c.name, "Status API");
    assert_eq!(c.count_requests(), 1);
    let status = request(c, "getStatus");
    assert_eq!(status.method, HttpMethod::Get);
    assert_eq!(status.url, "/status");
}

#[test]
fn import_json_document_and_rejections() {
    let json_doc = r#"{
        "openapi": "3.0.1",
        "info": { "title": "JSON Doc", "version": "1.0.0" },
        "paths": { "/a": { "get": { "operationId": "a" } } }
    }"#;
    let imported = openapi::detect_and_import(json_doc).unwrap();
    assert_eq!(imported.collections[0].name, "JSON Doc");

    let swagger = "swagger: '2.0'\ninfo: {title: x, version: '1'}\npaths: {}";
    let err = openapi::detect_and_import(swagger).unwrap_err();
    assert!(err.to_string().contains("2.0"), "{err}");
    assert!(matches!(
        err,
        restrust_interchange::InterchangeError::UnsupportedFormat(_)
    ));

    let v4 = "openapi: 4.0.0\ninfo: {title: x}\npaths: {}";
    assert!(openapi::detect_and_import(v4).is_err());
    assert!(openapi::detect_and_import("hello: world").is_err());
}

#[test]
fn export_3_0_smoke_round_trips_through_yaml() {
    let imported = openapi::detect_and_import(OPENAPI_3_0).unwrap();
    let doc = openapi::export_3_0(&imported.collections[0]).unwrap();

    assert!(doc.starts_with("# Generated by restrust"), "{doc}");

    let parsed: serde_json::Value = serde_yaml_ng::from_str(&doc).unwrap();
    let paths = parsed["paths"].as_object().unwrap();
    // the full URL path (including the server's /api/v1 prefix) becomes the
    // path key, since the exported document carries no servers
    assert_eq!(paths.len(), 2);
    assert_eq!(paths["/api/v1/pets"]["get"]["summary"], "listPets");
    assert_eq!(paths["/api/v1/pets"]["post"]["summary"], "createPet");
    assert_eq!(
        paths["/api/v1/pets/{petId}"]["get"]["summary"],
        "Info for a specific pet"
    );
    assert!(paths["/api/v1/pets"]["get"]["responses"]["default"].is_object());
}
