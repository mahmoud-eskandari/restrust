//! Postman collection (v2.1 / v3.0) and environment import/export.
//!
//! Import maps a Postman collection onto [`Collection`], recording every
//! approximation and drop in a [`Report`]. Parsing is done through
//! `serde_json::Value` for resilience against minor schema variations:
//! requests with unexpected extra fields still import, with the extra fields
//! listed in the report. Export produces a standards-shaped v2.1 collection
//! JSON that re-imports into an equivalent collection.

use std::collections::HashMap;

use serde::Serialize;
use serde_json::{json, Map, Value};

use restrust_core::model::{
    new_id, ApiKeyIn, AuthConfig, AuthKind, BodyKind, Collection, CollectionItem, Environment,
    EventListen, Folder, HttpMethod, KeyValue, RequestBody, RequestDef, ScriptEvent,
};

use crate::{description_from, Imported, InterchangeError, Report};

/// The v2.1 schema URL written into exports and used for version detection.
pub const V2_1_SCHEMA_URL: &str =
    "https://schema.getpostman.com/json/collection/v2.1.0/collection.json";

const KNOWN_ROOT_FIELDS: &[&str] = &["info", "item", "variable", "auth", "event"];
const KNOWN_INFO_FIELDS: &[&str] = &["name", "_postman_id", "description", "schema"];
const KNOWN_ITEM_FIELDS: &[&str] = &[
    "name",
    "description",
    "request",
    "item",
    "event",
    "response",
    "auth",
    "protocolProfileBehavior",
    "_postman_id",
    "id",
];
const KNOWN_REQUEST_FIELDS: &[&str] = &["method", "header", "body", "url", "auth", "description"];

// --- public API ---------------------------------------------------------------

/// Detects the Postman document flavor in `text` (JSON) and imports it:
/// v2.1 and v3.0 collections by their `info.schema` URL, plus standalone
/// Postman environment exports (`values[]`).
pub fn detect_and_import(text: &str) -> Result<Imported, InterchangeError> {
    let v: Value = serde_json::from_str(text)
        .map_err(|e| InterchangeError::BadInput(format!("not valid JSON: {e}")))?;

    // Standalone Postman environment export: {"name", "values": [...]}
    if v.get("info").is_none()
        && v.get("item").is_none()
        && v.get("items").is_none()
        && v.get("values").and_then(Value::as_array).is_some()
    {
        let env = environment_from(&v)?;
        let mut report = Report::default();
        let disabled = env.values.iter().filter(|row| !row.enabled).count();
        report.note_preserved(format!(
            "environment '{}': {} value(s) imported ({} disabled)",
            env.name,
            env.values.len(),
            disabled
        ));
        return Ok(Imported {
            collections: Vec::new(),
            environments: vec![env],
            report,
        });
    }

    let schema = v
        .pointer("/info/schema")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if schema.contains("/v2.1.0/") {
        import_v2_1_value(&v)
    } else if schema.contains("/v3.0.") {
        import_v3_value(&v)
    } else if schema.contains("/v2.0.") || schema.contains("/v1.") {
        Err(InterchangeError::UnsupportedFormat(format!(
            "Postman collection format '{schema}' is not supported; re-export from Postman as v2.1 or v3.0"
        )))
    } else if v.get("item").is_some() {
        let mut imported = import_v2_1_value(&v)?;
        imported
            .report
            .note_lossy("no info.schema present; imported assuming the v2.1 shape");
        Ok(imported)
    } else if v.get("items").is_some() {
        let mut imported = import_v3_value(&v)?;
        imported
            .report
            .note_lossy("no info.schema present; imported assuming the v3.0 shape");
        Ok(imported)
    } else {
        Err(InterchangeError::UnsupportedFormat(
            "unrecognized document: expected a Postman collection (info.schema) or environment (values[])".to_string(),
        ))
    }
}

/// Imports a Postman **v2.1** collection JSON file.
pub fn import_v2_1(text: &str) -> Result<Imported, InterchangeError> {
    let v: Value = serde_json::from_str(text)
        .map_err(|e| InterchangeError::BadInput(format!("not valid JSON: {e}")))?;
    import_v2_1_value(&v)
}

/// Imports a Postman **v3.0** collection JSON file (best effort; unmappable
/// parts are listed in the report).
pub fn import_v3(text: &str) -> Result<Imported, InterchangeError> {
    let v: Value = serde_json::from_str(text)
        .map_err(|e| InterchangeError::BadInput(format!("not valid JSON: {e}")))?;
    import_v3_value(&v)
}

/// Imports a standalone Postman environment JSON export (`values[]` rows
/// with `enabled` flags).
pub fn import_environment(text: &str) -> Result<Environment, InterchangeError> {
    let v: Value = serde_json::from_str(text)
        .map_err(|e| InterchangeError::BadInput(format!("not valid JSON: {e}")))?;
    environment_from(&v)
}

/// Exports a [`Collection`] as Postman **v2.1** collection JSON. The output
/// is shaped so that importing it again yields an equivalent collection.
pub fn export_v2_1(c: &Collection) -> Result<String, InterchangeError> {
    let out = OutCollection {
        info: OutInfo {
            name: c.name.clone(),
            _postman_id: c.id.clone(),
            description: c.description.clone(),
            schema: V2_1_SCHEMA_URL,
        },
        item: c.items.iter().map(item_to_out).collect(),
        variable: c
            .variables
            .iter()
            .map(|kv| OutVariable {
                key: kv.key.clone(),
                value: kv.value.clone(),
                disabled: (!kv.enabled).then_some(true),
            })
            .collect(),
        auth: c.auth.as_ref().map(auth_to_v21),
        event: c.events.iter().map(event_to_v21).collect(),
    };
    serde_json::to_string_pretty(&out).map_err(|e| InterchangeError::WithDetail {
        context: "Postman v2.1 export failed".to_string(),
        message: e.to_string(),
    })
}

// --- v2.1 import ---------------------------------------------------------------

fn import_v2_1_value(root: &Value) -> Result<Imported, InterchangeError> {
    let info = root
        .get("info")
        .and_then(Value::as_object)
        .ok_or_else(|| InterchangeError::BadInput("missing 'info' object".to_string()))?;
    let entries = root
        .get("item")
        .and_then(Value::as_array)
        .ok_or_else(|| InterchangeError::BadInput("missing 'item' array".to_string()))?;
    let mut report = Report::default();
    let collection = Collection {
        id: item_string(
            root.get("info").unwrap_or(&Value::Null),
            &["_postman_id", "id"],
        )
        .unwrap_or_else(new_id),
        name: info
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Imported collection")
            .to_string(),
        description: description_from(info.get("description")),
        items: items_from_v2_1(entries, &mut report),
        variables: kv_array(root.get("variable")),
        auth: root
            .get("auth")
            .and_then(|a| auth_from(a, "collection", &mut report)),
        events: events_from(root.get("event"), "collection", &mut report),
    };
    note_unknown_fields(Some(root), KNOWN_ROOT_FIELDS, "collection", &mut report);
    note_unknown_fields(
        root.get("info"),
        KNOWN_INFO_FIELDS,
        "collection info",
        &mut report,
    );
    Ok(Imported {
        collections: vec![collection],
        environments: Vec::new(),
        report,
    })
}

fn items_from_v2_1(entries: &[Value], report: &mut Report) -> Vec<CollectionItem> {
    let mut out = Vec::new();
    for entry in entries {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("(unnamed)")
            .to_string();
        note_unknown_fields(
            Some(entry),
            KNOWN_ITEM_FIELDS,
            &format!("item '{name}'"),
            report,
        );
        if entry.get("request").is_some() {
            if let Some(r) = request_def_from_v2_1(entry, report) {
                if let Some(responses) = entry.get("response").and_then(Value::as_array) {
                    if !responses.is_empty() {
                        report.note_unsupported(format!(
                            "request '{}': {} saved example response(s) are not modeled",
                            r.name,
                            responses.len()
                        ));
                    }
                }
                if entry.get("protocolProfileBehavior").is_some() {
                    report.note_unsupported(format!(
                        "request '{}': protocolProfileBehavior ignored",
                        r.name
                    ));
                }
                if entry
                    .get("event")
                    .and_then(Value::as_array)
                    .is_some_and(|a| !a.is_empty())
                {
                    report.note_unsupported(format!(
                        "request '{}': script events are not modeled on requests (collection-level events only); dropped",
                        r.name
                    ));
                }
                out.push(CollectionItem::Request(r));
            }
        } else if let Some(children) = entry.get("item").and_then(Value::as_array) {
            let folder = Folder {
                id: item_string(entry, &["_postman_id", "id"]).unwrap_or_else(new_id),
                name: name.clone(),
                description: description_from(entry.get("description")),
                items: items_from_v2_1(children, report),
                auth: entry
                    .get("auth")
                    .and_then(|a| auth_from(a, &format!("folder '{name}'"), report)),
            };
            if entry
                .get("event")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
            {
                report.note_unsupported(format!(
                    "folder '{}': script events are not modeled on folders; dropped",
                    folder.name
                ));
            }
            out.push(CollectionItem::Folder(folder));
        } else {
            report.note_unsupported(format!(
                "item '{name}': neither a request nor a folder; skipped"
            ));
        }
    }
    out
}

fn request_def_from_v2_1(item: &Value, report: &mut Report) -> Option<RequestDef> {
    let request = item.get("request")?;
    let name = item
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("request")
        .to_string();
    note_unknown_fields(
        Some(request),
        KNOWN_REQUEST_FIELDS,
        &format!("request '{name}'"),
        report,
    );
    let (method, url_value) = match request {
        Value::Object(o) => {
            let m = o.get("method").and_then(Value::as_str).unwrap_or("GET");
            match m.parse() {
                Ok(method) => (method, o.get("url").cloned().unwrap_or(Value::Null)),
                Err(_) => {
                    report.note_unsupported(format!(
                        "request '{name}': HTTP method '{m}' is not supported; request skipped"
                    ));
                    return None;
                }
            }
        }
        Value::String(s) => (HttpMethod::Get, Value::String(s.clone())),
        _ => {
            report.note_unsupported(format!(
                "request '{name}': malformed request object; skipped"
            ));
            return None;
        }
    };
    let (url, query) = url_from(&url_value, &name, report);
    Some(RequestDef {
        id: item_string(item, &["_postman_id", "id"]).unwrap_or_else(new_id),
        description: description_from(request.get("description"))
            .or_else(|| description_from(item.get("description"))),
        name: name.clone(),
        method,
        url,
        query,
        headers: kv_array(request.get("header")),
        body: request
            .get("body")
            .map(|b| body_from(b, &name, report))
            .unwrap_or_default(),
        auth: request
            .get("auth")
            .and_then(|a| auth_from(a, &format!("request '{name}'"), report)),
    })
}

// --- v3.0 import ---------------------------------------------------------------

fn import_v3_value(root: &Value) -> Result<Imported, InterchangeError> {
    let mut report = Report::default();
    let info = root.get("info");
    let name = info
        .and_then(|i| i.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("Imported Postman v3 collection")
        .to_string();
    if let Some(tag) = info
        .and_then(|i| i.pointer("/version/tag"))
        .and_then(Value::as_str)
    {
        report.note_lossy(format!("collection version tag '{tag}' is not modeled"));
    }
    let entries = root
        .get("items")
        .and_then(Value::as_array)
        .or_else(|| root.get("item").and_then(Value::as_array))
        .ok_or_else(|| {
            InterchangeError::BadInput("v3 collection has no 'items' array".to_string())
        })?;
    let collection = Collection {
        id: info
            .and_then(|i| item_string(i, &["_postman_id", "id"]))
            .unwrap_or_else(new_id),
        name,
        description: info.and_then(|i| description_from(i.get("description"))),
        items: items_from_v3(entries, &mut report),
        variables: variables_from_v3(root.get("variables")),
        auth: root
            .get("auth")
            .and_then(|a| auth_from(a, "collection", &mut report)),
        events: events_from(root.get("event"), "collection", &mut report),
    };
    note_unknown_fields(
        Some(root),
        &["info", "items", "item", "variables", "auth", "event"],
        "collection",
        &mut report,
    );
    Ok(Imported {
        collections: vec![collection],
        environments: Vec::new(),
        report,
    })
}

fn variables_from_v3(v: Option<&Value>) -> Vec<KeyValue> {
    match v {
        // v2-style array of {key, value, disabled}
        Some(Value::Array(rows)) => rows.iter().map(kv_from_row).collect(),
        // v3-style object map: {name: {value, disabled}}
        Some(Value::Object(map)) => map
            .iter()
            .map(|(k, v)| KeyValue {
                key: k.clone(),
                value: v
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                enabled: !v.get("disabled").and_then(Value::as_bool).unwrap_or(false),
                description: description_from(v.get("description")),
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn items_from_v3(entries: &[Value], report: &mut Report) -> Vec<CollectionItem> {
    let mut out = Vec::new();
    for entry in entries {
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("(unnamed)")
            .to_string();
        let kind = entry
            .get("kind")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| {
                if entry.get("request").is_some() {
                    "request".to_string()
                } else if entry.get("items").is_some() || entry.get("item").is_some() {
                    "folder".to_string()
                } else {
                    String::new()
                }
            });
        match kind.as_str() {
            "request" => {
                // Structured ({kind, name, request}) or flattened (fields
                // directly on the item) forms are both accepted.
                let request = entry.get("request").unwrap_or(entry);
                if let Some(r) = request_def_from_v3(entry, request, report) {
                    out.push(CollectionItem::Request(r));
                }
            }
            "folder" => {
                let children: &[Value] = entry
                    .get("items")
                    .and_then(Value::as_array)
                    .or_else(|| entry.get("item").and_then(Value::as_array))
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                out.push(CollectionItem::Folder(Folder {
                    id: item_string(entry, &["_postman_id", "id"]).unwrap_or_else(new_id),
                    name: name.clone(),
                    description: description_from(entry.get("description")),
                    items: items_from_v3(children, report),
                    auth: entry
                        .get("auth")
                        .and_then(|a| auth_from(a, &format!("folder '{name}'"), report)),
                }));
            }
            other => report.note_unsupported(format!(
                "item '{name}': kind '{other}' is not supported; skipped"
            )),
        }
    }
    out
}

fn request_def_from_v3(entry: &Value, request: &Value, report: &mut Report) -> Option<RequestDef> {
    let name = entry
        .get("name")
        .and_then(Value::as_str)
        .or_else(|| request.get("name").and_then(Value::as_str))
        .unwrap_or("request")
        .to_string();
    let method_str = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("GET");
    let method = match method_str.parse() {
        Ok(m) => m,
        Err(_) => {
            report.note_unsupported(format!(
                "request '{name}': HTTP method '{method_str}' is not supported; request skipped"
            ));
            return None;
        }
    };
    let (url, query) = url_from(
        &request.get("url").cloned().unwrap_or(Value::Null),
        &name,
        report,
    );
    Some(RequestDef {
        id: item_string(entry, &["_postman_id", "id"]).unwrap_or_else(new_id),
        description: description_from(request.get("description"))
            .or_else(|| description_from(entry.get("description"))),
        name: name.clone(),
        method,
        url,
        query,
        headers: kv_array(request.get("header")),
        body: request
            .get("body")
            .map(|b| body_from(b, &name, report))
            .unwrap_or_default(),
        auth: request
            .get("auth")
            .and_then(|a| auth_from(a, &format!("request '{name}'"), report)),
    })
}

// --- shared converters ---------------------------------------------------------

fn item_string(v: &Value, fields: &[&str]) -> Option<String> {
    fields
        .iter()
        .find_map(|f| v.get(*f).and_then(Value::as_str).map(str::to_string))
        .filter(|s| !s.is_empty())
}

fn note_unknown_fields(obj: Option<&Value>, known: &[&str], owner: &str, report: &mut Report) {
    let Some(Value::Object(o)) = obj else { return };
    let unknown: Vec<&str> = o
        .keys()
        .filter(|k| !known.contains(&k.as_str()))
        .map(|k| k.as_str())
        .collect();
    if !unknown.is_empty() {
        report.note_unsupported(format!(
            "{owner}: unsupported fields ignored: {}",
            unknown.join(", ")
        ));
    }
}

fn kv_from_row(row: &Value) -> KeyValue {
    KeyValue {
        key: row
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        value: row
            .get("value")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        enabled: !row
            .get("disabled")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        description: description_from(row.get("description")),
    }
}

fn kv_array(v: Option<&Value>) -> Vec<KeyValue> {
    v.and_then(Value::as_array)
        .map(|rows| rows.iter().map(kv_from_row).collect())
        .unwrap_or_default()
}

/// Maps a Postman `url` (plain string or structured object) to the base URL
/// plus separate query rows. Path variables (`:id` / `{id}` with a
/// `url.variable` value) are inlined and noted.
fn url_from(v: &Value, req_name: &str, report: &mut Report) -> (String, Vec<KeyValue>) {
    let Value::Object(o) = v else {
        return (v.as_str().unwrap_or_default().to_string(), Vec::new());
    };
    let query = kv_array(o.get("query"));
    let path_vars: HashMap<String, String> = o
        .get("variable")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    let k = row.get("key").and_then(Value::as_str)?;
                    let val = row.get("value").and_then(Value::as_str).unwrap_or_default();
                    Some((k.to_string(), val.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    let mut base = String::new();
    if let Some(protocol) = o.get("protocol").and_then(Value::as_str) {
        base.push_str(protocol);
        base.push_str("://");
    }
    match o.get("host") {
        Some(Value::Array(parts)) => {
            base.push_str(
                &parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("."),
            );
        }
        Some(Value::String(s)) => base.push_str(s),
        _ => {}
    }
    match o.get("port") {
        Some(Value::String(p)) if !p.is_empty() => {
            base.push(':');
            base.push_str(p);
        }
        Some(Value::Number(n)) => {
            base.push(':');
            base.push_str(&n.to_string());
        }
        _ => {}
    }
    if let Some(Value::Array(segs)) = o.get("path") {
        let joined: Vec<String> = segs
            .iter()
            .filter_map(Value::as_str)
            .map(|seg| {
                let mut seg = seg.to_string();
                for (k, val) in &path_vars {
                    if seg == format!(":{k}") || seg == format!("{{{k}}}") {
                        report.note_lossy(format!(
                            "request '{req_name}': path variable '{k}' inlined as '{val}'"
                        ));
                        seg = val.clone();
                    } else if seg.contains(&format!("{{{k}}}")) {
                        seg = seg.replace(&format!("{{{k}}}"), val);
                    }
                }
                seg
            })
            .collect();
        if !joined.is_empty() {
            base.push('/');
            base.push_str(&joined.join("/"));
        }
    }
    if base.is_empty() {
        // No structured parts at all: fall back to the raw string. When
        // query rows were given separately, strip the raw query string so it
        // is not represented twice.
        if let Some(raw) = o.get("raw").and_then(Value::as_str) {
            base = if query.is_empty() {
                raw.to_string()
            } else {
                raw.split('?').next().unwrap_or(raw).to_string()
            };
        }
    }
    (base, query)
}

/// Maps a Postman `options.raw.language` to a MIME type.
fn raw_language_mime(lang: &str) -> Option<&'static str> {
    match lang {
        "json" => Some("application/json"),
        "text" => Some("text/plain"),
        "javascript" => Some("application/javascript"),
        "html" => Some("text/html"),
        "xml" => Some("application/xml"),
        _ => None,
    }
}

/// Reverse of [`raw_language_mime`].
fn mime_language(mime: &str) -> Option<&'static str> {
    match mime {
        "application/json" => Some("json"),
        "text/plain" => Some("text"),
        "application/javascript" => Some("javascript"),
        "text/html" => Some("html"),
        "application/xml" => Some("xml"),
        _ => None,
    }
}

fn body_from(body: &Value, req_name: &str, report: &mut Report) -> RequestBody {
    let mode = body.get("mode").and_then(Value::as_str).unwrap_or_default();
    match mode {
        "raw" => {
            let lang = body
                .pointer("/options/raw/language")
                .and_then(Value::as_str)
                .unwrap_or("text");
            let mime = raw_language_mime(lang).unwrap_or("text/plain").to_string();
            RequestBody {
                kind: BodyKind::Raw { mime },
                raw: Some(
                    body.get("raw")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                ),
                rows: Vec::new(),
            }
        }
        "urlencoded" => RequestBody {
            kind: BodyKind::UrlEncoded,
            raw: None,
            rows: kv_array(body.get("urlencoded")),
        },
        "formdata" => {
            let mut rows = Vec::new();
            if let Some(arr) = body.get("formdata").and_then(Value::as_array) {
                for p in arr {
                    let key = p
                        .get("key")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let is_file = p.get("type").and_then(Value::as_str) == Some("file");
                    let value = if is_file {
                        let src = p
                            .get("src")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string();
                        report.note_lossy(format!(
                            "request '{req_name}': form-data file '{src}' preserved as name only"
                        ));
                        src
                    } else {
                        p.get("value")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string()
                    };
                    rows.push(KeyValue {
                        key,
                        value,
                        enabled: !p.get("disabled").and_then(Value::as_bool).unwrap_or(false),
                        description: description_from(p.get("description")),
                    });
                }
            }
            RequestBody {
                kind: BodyKind::Multipart,
                raw: None,
                rows,
            }
        }
        "file" => RequestBody {
            kind: BodyKind::Binary {
                file_path: body
                    .pointer("/file/src")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            },
            raw: None,
            rows: Vec::new(),
        },
        "" => RequestBody::default(),
        other => {
            report.note_unsupported(format!(
                "request '{req_name}': body mode '{other}' is not supported"
            ));
            RequestBody::default()
        }
    }
}

fn auth_from(auth: &Value, owner: &str, report: &mut Report) -> Option<AuthConfig> {
    let typ = auth.get("type")?.as_str()?;
    let rows = |name: &str| -> HashMap<String, String> {
        auth.get(name)
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|r| {
                        Some((
                            r.get("key")?.as_str()?.to_string(),
                            r.get("value")
                                .and_then(Value::as_str)
                                .unwrap_or_default()
                                .to_string(),
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default()
    };
    match typ {
        "bearer" => Some(AuthConfig {
            kind: AuthKind::Bearer {
                token: rows("bearer").get("token").cloned().unwrap_or_default(),
            },
        }),
        "basic" => {
            let m = rows("basic");
            Some(AuthConfig {
                kind: AuthKind::Basic {
                    username: m.get("username").cloned().unwrap_or_default(),
                    password: m.get("password").cloned().unwrap_or_default(),
                },
            })
        }
        "apikey" => {
            let m = rows("apikey");
            let add_to = match m.get("in").map(String::as_str) {
                Some("query") => ApiKeyIn::Query,
                _ => ApiKeyIn::Header,
            };
            Some(AuthConfig {
                kind: AuthKind::ApiKey {
                    key: m.get("key").cloned().unwrap_or_default(),
                    value: m.get("value").cloned().unwrap_or_default(),
                    add_to,
                },
            })
        }
        "noauth" => Some(AuthConfig {
            kind: AuthKind::None,
        }),
        other => {
            report.note_unsupported(format!(
                "{owner}: auth type '{other}' is not supported; no auth applied"
            ));
            None
        }
    }
}

fn events_from(events: Option<&Value>, owner: &str, report: &mut Report) -> Vec<ScriptEvent> {
    let Some(arr) = events.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for ev in arr {
        let listen = ev.get("listen").and_then(Value::as_str).unwrap_or_default();
        let listen_kind = match listen {
            "prerequest" => EventListen::PreRequest,
            "test" => EventListen::PostResponse,
            other => {
                report.note_unsupported(format!(
                    "{owner}: script event '{other}' is not modeled; skipped"
                ));
                continue;
            }
        };
        let script_text = match ev.get("script") {
            Some(Value::Object(s)) => match s.get("exec") {
                Some(Value::Array(lines)) => lines
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n"),
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            },
            _ => String::new(),
        };
        report.note_preserved(format!(
            "{owner}: {listen} script preserved as text (scripts are not executed in v1)"
        ));
        out.push(ScriptEvent {
            listen: listen_kind,
            script: script_text,
        });
    }
    out
}

fn environment_from(v: &Value) -> Result<Environment, InterchangeError> {
    let values = v.get("values").and_then(Value::as_array).ok_or_else(|| {
        InterchangeError::BadInput("environment has no 'values' array".to_string())
    })?;
    Ok(Environment {
        id: item_string(v, &["id", "_postman_id"]).unwrap_or_else(new_id),
        name: v
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Imported environment")
            .to_string(),
        values: values
            .iter()
            .map(|row| KeyValue {
                key: row
                    .get("key")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                value: row
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                enabled: row.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                description: description_from(row.get("description")),
            })
            .collect(),
    })
}

// --- v2.1 export ---------------------------------------------------------------

#[derive(Serialize)]
struct OutInfo {
    name: String,
    _postman_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    schema: &'static str,
}

#[derive(Serialize)]
struct OutVariable {
    key: String,
    value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    disabled: Option<bool>,
}

#[derive(Serialize)]
struct OutRequest {
    method: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    header: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<Value>,
    url: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    auth: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[derive(Serialize)]
struct OutItem {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    request: Option<OutRequest>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    item: Vec<OutItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auth: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    event: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[derive(Serialize)]
struct OutCollection {
    info: OutInfo,
    item: Vec<OutItem>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    variable: Vec<OutVariable>,
    #[serde(skip_serializing_if = "Option::is_none")]
    auth: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    event: Vec<Value>,
}

fn item_to_out(item: &CollectionItem) -> OutItem {
    match item {
        CollectionItem::Request(r) => OutItem {
            name: r.name.clone(),
            request: Some(OutRequest {
                method: r.method.as_str().to_string(),
                header: r.headers.iter().map(row_to_v21).collect(),
                body: body_to_v21(&r.body),
                url: url_to_v21(&r.url, &r.query),
                auth: r.auth.as_ref().map(auth_to_v21),
                description: r.description.clone(),
            }),
            item: Vec::new(),
            auth: None,
            event: Vec::new(),
            description: None,
        },
        CollectionItem::Folder(f) => OutItem {
            name: f.name.clone(),
            request: None,
            item: f.items.iter().map(item_to_out).collect(),
            auth: f.auth.as_ref().map(auth_to_v21),
            event: Vec::new(),
            description: f.description.clone(),
        },
    }
}

fn row_to_v21(kv: &KeyValue) -> Value {
    let mut m = Map::new();
    m.insert("key".into(), json!(kv.key));
    m.insert("value".into(), json!(kv.value));
    if !kv.enabled {
        m.insert("disabled".into(), json!(true));
    }
    if let Some(d) = &kv.description {
        m.insert("description".into(), json!(d));
    }
    Value::Object(m)
}

/// Builds the Postman url value. With no query rows the URL is exported as a
/// plain string (guarantees verbatim round-trips); with query rows it is
/// exported as a structured object with raw/host/path/query.
fn url_to_v21(url: &str, query: &[KeyValue]) -> Value {
    if query.is_empty() {
        return json!(url);
    }
    let raw = raw_with_query(url, query);
    let mut obj = Map::new();
    obj.insert("raw".into(), json!(raw));
    // Strip any query string before splitting into host/path parts.
    let base = url.split('?').next().unwrap_or(url);
    if let Some(pos) = base.find("://") {
        let protocol = &base[..pos];
        let protocol_ok = !protocol.is_empty()
            && protocol
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.');
        if protocol_ok {
            let rest = &base[pos + 3..];
            let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
            obj.insert("protocol".into(), json!(protocol));
            let (host, port) = match authority.rsplit_once(':') {
                Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => {
                    (h, Some(p))
                }
                _ => (authority, None),
            };
            obj.insert(
                "host".into(),
                Value::Array(host.split('.').map(|s| json!(s)).collect()),
            );
            if let Some(p) = port {
                obj.insert("port".into(), json!(p));
            }
            if !path.is_empty() || rest.ends_with('/') {
                obj.insert(
                    "path".into(),
                    Value::Array(path.split('/').map(|s| json!(s)).collect()),
                );
            }
        } else {
            split_schemeless(&mut obj, base);
        }
    } else {
        split_schemeless(&mut obj, base);
    }
    obj.insert(
        "query".into(),
        Value::Array(query.iter().map(row_to_v21).collect()),
    );
    Value::Object(obj)
}

/// Splits a scheme-less base (`{{host}}/path`, `host:8080/x`, `/rel`) into
/// host/path parts for the structured url object.
fn split_schemeless(obj: &mut Map<String, Value>, base: &str) {
    match base.split_once('/') {
        Some((host, path)) => {
            obj.insert("host".into(), Value::Array(vec![json!(host)]));
            if !path.is_empty() {
                obj.insert(
                    "path".into(),
                    Value::Array(path.split('/').map(|s| json!(s)).collect()),
                );
            }
        }
        None => {
            obj.insert("host".into(), Value::Array(vec![json!(base)]));
        }
    }
}

fn raw_with_query(url: &str, query: &[KeyValue]) -> String {
    let enabled: Vec<String> = query
        .iter()
        .filter(|q| q.enabled)
        .map(|q| format!("{}={}", q.key, q.value))
        .collect();
    if enabled.is_empty() {
        return url.to_string();
    }
    let sep = if url.contains('?') { '&' } else { '?' };
    format!("{url}{sep}{}", enabled.join("&"))
}

fn body_to_v21(body: &RequestBody) -> Option<Value> {
    match &body.kind {
        BodyKind::None => None,
        BodyKind::Raw { mime } => {
            let mut m = Map::new();
            m.insert("mode".into(), json!("raw"));
            m.insert("raw".into(), json!(body.raw.clone().unwrap_or_default()));
            if let Some(lang) = mime_language(mime) {
                m.insert("options".into(), json!({ "raw": { "language": lang } }));
            }
            Some(Value::Object(m))
        }
        BodyKind::UrlEncoded => Some(json!({
            "mode": "urlencoded",
            "urlencoded": body.rows.iter().map(row_to_v21).collect::<Vec<_>>(),
        })),
        BodyKind::Multipart => {
            // The core model does not distinguish file rows, so every row is
            // exported as a text form-data parameter.
            let rows: Vec<Value> = body
                .rows
                .iter()
                .map(|kv| {
                    let mut v = row_to_v21(kv);
                    v["type"] = json!("text");
                    v
                })
                .collect();
            Some(json!({ "mode": "formdata", "formdata": rows }))
        }
        BodyKind::Binary { file_path } => {
            Some(json!({ "mode": "file", "file": { "src": file_path } }))
        }
    }
}

fn auth_to_v21(a: &AuthConfig) -> Value {
    let row = |key: &str, value: &str| json!({ "key": key, "value": value, "type": "string" });
    match &a.kind {
        AuthKind::None => json!({ "type": "noauth" }),
        AuthKind::Bearer { token } => json!({
            "type": "bearer",
            "bearer": [row("token", token)],
        }),
        AuthKind::Basic { username, password } => json!({
            "type": "basic",
            "basic": [row("username", username), row("password", password)],
        }),
        AuthKind::ApiKey { key, value, add_to } => {
            let add_to = match add_to {
                ApiKeyIn::Header => "header",
                ApiKeyIn::Query => "query",
            };
            json!({
                "type": "apikey",
                "apikey": [row("key", key), row("value", value), row("in", add_to)],
            })
        }
    }
}

fn event_to_v21(e: &ScriptEvent) -> Value {
    let listen = match e.listen {
        EventListen::PreRequest => "prerequest",
        EventListen::PostResponse => "test",
    };
    json!({
        "listen": listen,
        "script": {
            "type": "text/javascript",
            "exec": e.script.split('\n').collect::<Vec<_>>(),
        }
    })
}
