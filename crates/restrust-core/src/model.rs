//! Domain model for collections, requests, environments, and history.
//!
//! All types serialize to camelCase JSON (the storage and wire format).
//! Ids are UUID v4 strings ([`new_id`]); timestamps are RFC3339 UTC strings.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Generates a fresh UUID v4 string — the id format used by every model entity.
pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}

/// Current time as an RFC3339 UTC string, e.g. `2026-10-20T12:34:56Z`.
pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// HTTP methods supported by a [`RequestDef`].
///
/// Serde form is camelCase (`"get"`, `"post"`, ...); [`Display`](fmt::Display)
/// and [`HttpMethod::as_str`] use the conventional uppercase form (`GET`), and
/// [`FromStr`] accepts any case.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
}

impl HttpMethod {
    pub fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Patch => "PATCH",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Head => "HEAD",
            HttpMethod::Options => "OPTIONS",
        }
    }
}

impl fmt::Display for HttpMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for HttpMethod {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_uppercase().as_str() {
            "GET" => Ok(HttpMethod::Get),
            "POST" => Ok(HttpMethod::Post),
            "PUT" => Ok(HttpMethod::Put),
            "PATCH" => Ok(HttpMethod::Patch),
            "DELETE" => Ok(HttpMethod::Delete),
            "HEAD" => Ok(HttpMethod::Head),
            "OPTIONS" => Ok(HttpMethod::Options),
            other => Err(format!("unknown HTTP method: {other}")),
        }
    }
}

/// A single query/header/body/variable row.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyValue {
    pub key: String,
    pub value: String,
    pub enabled: bool,
    pub description: Option<String>,
}

impl KeyValue {
    /// Convenience constructor: an enabled row without a description.
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        KeyValue {
            key: key.into(),
            value: value.into(),
            enabled: true,
            description: None,
        }
    }
}

/// Where an API-key auth value is attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApiKeyIn {
    Header,
    Query,
}

/// One authentication scheme. `None` is an *explicit* "no auth" choice and, as
/// a defined value, overrides any inherited auth (see
/// [`Collection::resolve_request_auth`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AuthKind {
    None,
    Bearer {
        token: String,
    },
    Basic {
        username: String,
        password: String,
    },
    ApiKey {
        key: String,
        value: String,
        add_to: ApiKeyIn,
    },
}

/// Wrapper carrying an auth choice on a request/folder/collection. An
/// `Option<AuthConfig>` of `None` means "not defined here" and therefore
/// inherits from the enclosing scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthConfig {
    pub kind: AuthKind,
}

/// The kind of payload a request carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum BodyKind {
    None,
    Raw { mime: String },
    UrlEncoded,
    Multipart,
    Binary { file_path: String },
}

/// A request body: the kind plus its payload (`raw` for raw bodies, `rows` for
/// urlencoded/multipart form rows).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestBody {
    pub kind: BodyKind,
    pub raw: Option<String>,
    pub rows: Vec<KeyValue>,
}

impl Default for RequestBody {
    fn default() -> Self {
        RequestBody {
            kind: BodyKind::None,
            raw: None,
            rows: Vec::new(),
        }
    }
}

/// A single saved request definition.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestDef {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub method: HttpMethod,
    pub url: String,
    pub query: Vec<KeyValue>,
    pub headers: Vec<KeyValue>,
    pub body: RequestBody,
    pub auth: Option<AuthConfig>,
}

impl RequestDef {
    /// Convenience constructor: fresh id, empty rows, no auth, no body.
    pub fn new(method: HttpMethod, name: impl Into<String>, url: impl Into<String>) -> Self {
        RequestDef {
            id: new_id(),
            name: name.into(),
            description: None,
            method,
            url: url.into(),
            query: Vec::new(),
            headers: Vec::new(),
            body: RequestBody::default(),
            auth: None,
        }
    }
}

/// A folder grouping requests and nested folders inside a collection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub items: Vec<CollectionItem>,
    pub auth: Option<AuthConfig>,
}

/// An item of a collection tree: either a request or a folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CollectionItem {
    Request(RequestDef),
    Folder(Folder),
}

impl CollectionItem {
    pub fn name(&self) -> &str {
        match self {
            CollectionItem::Request(r) => &r.name,
            CollectionItem::Folder(f) => &f.name,
        }
    }

    pub fn id(&self) -> &str {
        match self {
            CollectionItem::Request(r) => &r.id,
            CollectionItem::Folder(f) => &f.id,
        }
    }

    /// Renames this node regardless of kind.
    pub fn rename(&mut self, new_name: String) {
        match self {
            CollectionItem::Request(r) => r.name = new_name,
            CollectionItem::Folder(f) => f.name = new_name,
        }
    }

    /// Downcasts to the request definition, if this item is a request.
    pub fn as_request(&self) -> Option<&RequestDef> {
        match self {
            CollectionItem::Request(r) => Some(r),
            CollectionItem::Folder(_) => None,
        }
    }
}

/// When a script event fires. Scripts are preserved as text only — restrust v1
/// does not execute them (documented limitation).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EventListen {
    PreRequest,
    PostResponse,
}

/// A pre-request or post-response script, preserved verbatim from imports.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptEvent {
    pub listen: EventListen,
    pub script: String,
}

/// A collection: the top-level tree of requests and folders, plus collection
/// variables, collection-level auth, and script events.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub items: Vec<CollectionItem>,
    pub variables: Vec<KeyValue>,
    pub auth: Option<AuthConfig>,
    pub events: Vec<ScriptEvent>,
}

impl Collection {
    /// Convenience constructor: fresh id, empty items/variables/events, no auth.
    pub fn new(name: impl Into<String>) -> Self {
        Collection {
            id: new_id(),
            name: name.into(),
            description: None,
            items: Vec::new(),
            variables: Vec::new(),
            auth: None,
            events: Vec::new(),
        }
    }

    /// Finds a request anywhere in the tree by id.
    pub fn find_request(&self, id: &str) -> Option<&RequestDef> {
        find_request_in(&self.items, id)
    }

    /// Finds a request anywhere in the tree by id, mutably.
    pub fn find_request_mut(&mut self, id: &str) -> Option<&mut RequestDef> {
        find_request_mut_in(&mut self.items, id)
    }

    /// Finds any tree node (request or folder) by id, mutably.
    pub fn find_item_mut(&mut self, id: &str) -> Option<&mut CollectionItem> {
        find_item_mut_in(&mut self.items, id)
    }

    /// Finds a folder anywhere in the tree by id, mutably.
    pub fn find_folder_mut(&mut self, id: &str) -> Option<&mut Folder> {
        find_folder_mut_in(&mut self.items, id)
    }

    /// Inserts an item under a parent folder id, or into the collection root
    /// when `parent_id` is `None`. Returns `false` (without inserting) when
    /// `parent_id` names no folder in this collection.
    pub fn insert_item(&mut self, parent_id: Option<&str>, item: CollectionItem) -> bool {
        match parent_id {
            None => {
                self.items.push(item);
                true
            }
            Some(folder_id) => match self.find_folder_mut(folder_id) {
                Some(folder) => {
                    folder.items.push(item);
                    true
                }
                None => false,
            },
        }
    }

    /// Removes and returns the tree node with the given id, searching the root
    /// and every folder. Returns `None` when the id is not present.
    pub fn remove_item(&mut self, id: &str) -> Option<CollectionItem> {
        remove_item_from(&mut self.items, id)
    }

    /// Counts all requests in the tree, at any depth.
    pub fn count_requests(&self) -> usize {
        count_requests_in(&self.items)
    }

    /// Resolves the effective auth for a request: the nearest *defined* auth
    /// wins — `request.auth`, else the innermost enclosing folder's `auth`, else
    /// outer folders, else `collection.auth`; when nothing is defined (or the
    /// request id is unknown) the result is [`AuthKind::None`]. An explicit
    /// `AuthKind::None` at any level suppresses inheritance.
    pub fn resolve_request_auth(&self, request_id: &str) -> AuthKind {
        let inherited = self.auth.as_ref().map(|a| &a.kind);
        resolve_auth_in(&self.items, request_id, inherited).unwrap_or(AuthKind::None)
    }
}

/// A named set of variable rows selectable in the client.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Environment {
    pub id: String,
    pub name: String,
    pub values: Vec<KeyValue>,
}

/// Summary of a received response (the body itself is not persisted here).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseSummary {
    pub status: u16,
    pub reason: String,
    pub size: u64,
    pub duration_ms: u64,
    /// RFC3339 UTC string.
    pub time: String,
}

/// One executed request in the history log.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub id: String,
    /// RFC3339 UTC string.
    pub time: String,
    pub request: RequestDef,
    pub response: Option<ResponseSummary>,
}

// --- private tree walkers ---------------------------------------------------

fn find_request_in<'a>(items: &'a [CollectionItem], id: &str) -> Option<&'a RequestDef> {
    for item in items {
        match item {
            CollectionItem::Request(r) if r.id == id => return Some(r),
            CollectionItem::Folder(f) => {
                if let Some(found) = find_request_in(&f.items, id) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_request_mut_in<'a>(
    items: &'a mut [CollectionItem],
    id: &str,
) -> Option<&'a mut RequestDef> {
    for item in items {
        match item {
            CollectionItem::Request(r) if r.id == id => return Some(r),
            CollectionItem::Folder(f) => {
                if let Some(found) = find_request_mut_in(&mut f.items, id) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_item_mut_in<'a>(
    items: &'a mut [CollectionItem],
    id: &str,
) -> Option<&'a mut CollectionItem> {
    for item in items {
        if item.id() == id {
            return Some(item);
        }
        if let CollectionItem::Folder(f) = item {
            if let Some(found) = find_item_mut_in(&mut f.items, id) {
                return Some(found);
            }
        }
    }
    None
}

fn find_folder_mut_in<'a>(items: &'a mut [CollectionItem], id: &str) -> Option<&'a mut Folder> {
    for item in items {
        if let CollectionItem::Folder(f) = item {
            if f.id == id {
                return Some(f);
            }
            if let Some(found) = find_folder_mut_in(&mut f.items, id) {
                return Some(found);
            }
        }
    }
    None
}

fn remove_item_from(items: &mut Vec<CollectionItem>, id: &str) -> Option<CollectionItem> {
    if let Some(pos) = items.iter().position(|item| item.id() == id) {
        return Some(items.remove(pos));
    }
    for item in items.iter_mut() {
        if let CollectionItem::Folder(f) = item {
            if let Some(removed) = remove_item_from(&mut f.items, id) {
                return Some(removed);
            }
        }
    }
    None
}

fn count_requests_in(items: &[CollectionItem]) -> usize {
    items
        .iter()
        .map(|item| match item {
            CollectionItem::Request(_) => 1,
            CollectionItem::Folder(f) => count_requests_in(&f.items),
        })
        .sum()
}

/// Walks the tree carrying the auth defined by enclosing scopes (innermost
/// wins, already folded into `inherited`). Returns `None` when the request id
/// is not in this subtree.
fn resolve_auth_in(
    items: &[CollectionItem],
    request_id: &str,
    inherited: Option<&AuthKind>,
) -> Option<AuthKind> {
    for item in items {
        match item {
            CollectionItem::Request(r) if r.id == request_id => {
                return Some(
                    r.auth
                        .as_ref()
                        .map(|a| a.kind.clone())
                        .or_else(|| inherited.cloned())
                        .unwrap_or(AuthKind::None),
                );
            }
            CollectionItem::Folder(f) => {
                let inner = f.auth.as_ref().map(|a| &a.kind).or(inherited);
                if let Some(found) = resolve_auth_in(&f.items, request_id, inner) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(id: &str, name: &str, method: HttpMethod, auth: Option<AuthConfig>) -> RequestDef {
        RequestDef {
            id: id.to_string(),
            name: name.to_string(),
            description: None,
            method,
            url: format!("https://api.example.test/{name}"),
            query: Vec::new(),
            headers: Vec::new(),
            body: RequestBody::default(),
            auth,
        }
    }

    /// A collection exercising every enum variant of the model.
    fn sample_collection() -> Collection {
        Collection {
            id: "c1".to_string(),
            name: "Sample API".to_string(),
            description: Some("kitchen sink".to_string()),
            variables: vec![KeyValue::new("base_url", "https://api.example.test")],
            auth: Some(AuthConfig {
                kind: AuthKind::Bearer {
                    token: "collection-token".to_string(),
                },
            }),
            events: vec![
                ScriptEvent {
                    listen: EventListen::PreRequest,
                    script: "console.log('pre')".to_string(),
                },
                ScriptEvent {
                    listen: EventListen::PostResponse,
                    script: "pm.test('ok', () => true)".to_string(),
                },
            ],
            items: vec![
                CollectionItem::Request(request("r1", "list", HttpMethod::Get, None)),
                CollectionItem::Folder(Folder {
                    id: "f1".to_string(),
                    name: "Auth flows".to_string(),
                    description: Some("nested".to_string()),
                    auth: Some(AuthConfig {
                        kind: AuthKind::Basic {
                            username: "user".to_string(),
                            password: "pass".to_string(),
                        },
                    }),
                    items: vec![
                        CollectionItem::Request(RequestDef {
                            id: "r2".to_string(),
                            name: "login".to_string(),
                            description: Some("raw json body".to_string()),
                            method: HttpMethod::Post,
                            url: "{{base_url}}/login".to_string(),
                            query: vec![KeyValue::new("verbose", "true")],
                            headers: vec![KeyValue {
                                key: "Accept".to_string(),
                                value: "application/json".to_string(),
                                enabled: true,
                                description: Some("what to accept".to_string()),
                            }],
                            body: RequestBody {
                                kind: BodyKind::Raw {
                                    mime: "application/json".to_string(),
                                },
                                raw: Some("{\"user\":1}".to_string()),
                                rows: Vec::new(),
                            },
                            auth: Some(AuthConfig {
                                kind: AuthKind::ApiKey {
                                    key: "X-Api-Key".to_string(),
                                    value: "secret".to_string(),
                                    add_to: ApiKeyIn::Query,
                                },
                            }),
                        }),
                        CollectionItem::Request(RequestDef {
                            id: "r3".to_string(),
                            name: "update".to_string(),
                            description: None,
                            method: HttpMethod::Put,
                            url: "{{base_url}}/things/1".to_string(),
                            query: Vec::new(),
                            headers: Vec::new(),
                            body: RequestBody {
                                kind: BodyKind::UrlEncoded,
                                raw: None,
                                rows: vec![KeyValue::new("a", "1"), KeyValue::new("b", "2")],
                            },
                            auth: None,
                        }),
                        CollectionItem::Folder(Folder {
                            id: "f2".to_string(),
                            name: "deep".to_string(),
                            description: None,
                            items: vec![CollectionItem::Request(RequestDef {
                                id: "r4".to_string(),
                                name: "upload".to_string(),
                                description: None,
                                method: HttpMethod::Patch,
                                url: "{{base_url}}/upload".to_string(),
                                query: Vec::new(),
                                headers: Vec::new(),
                                body: RequestBody {
                                    kind: BodyKind::Binary {
                                        file_path: "/tmp/blob.bin".to_string(),
                                    },
                                    raw: None,
                                    rows: Vec::new(),
                                },
                                auth: None,
                            })],
                            auth: None,
                        }),
                    ],
                }),
                CollectionItem::Request(RequestDef {
                    id: "r5".to_string(),
                    name: "multipart".to_string(),
                    description: None,
                    method: HttpMethod::Delete,
                    url: "{{base_url}}/multi".to_string(),
                    query: Vec::new(),
                    headers: Vec::new(),
                    body: RequestBody {
                        kind: BodyKind::Multipart,
                        raw: None,
                        rows: vec![KeyValue::new("field", "value")],
                    },
                    auth: None,
                }),
                CollectionItem::Request(request("r6", "head", HttpMethod::Head, None)),
                CollectionItem::Request(request("r7", "options", HttpMethod::Options, None)),
            ],
        }
    }

    #[test]
    fn http_method_display_as_str_and_parse() {
        assert_eq!(HttpMethod::Get.to_string(), "GET");
        assert_eq!(HttpMethod::Patch.as_str(), "PATCH");
        for method in [
            HttpMethod::Get,
            HttpMethod::Post,
            HttpMethod::Put,
            HttpMethod::Patch,
            HttpMethod::Delete,
            HttpMethod::Head,
            HttpMethod::Options,
        ] {
            assert_eq!(method.as_str().parse::<HttpMethod>().unwrap(), method);
            assert_eq!(
                method
                    .as_str()
                    .to_ascii_lowercase()
                    .parse::<HttpMethod>()
                    .unwrap(),
                method
            );
        }
        assert!("FETCH".parse::<HttpMethod>().is_err());
    }

    #[test]
    fn http_method_serde_is_camel_case() {
        assert_eq!(
            serde_json::to_string(&HttpMethod::Get).unwrap(),
            "\"get\"".to_string()
        );
        let method: HttpMethod = serde_json::from_str("\"delete\"").unwrap();
        assert_eq!(method, HttpMethod::Delete);
    }

    #[test]
    fn collection_json_roundtrip() {
        let collection = sample_collection();
        let json = serde_json::to_string(&collection).unwrap();
        let back: Collection = serde_json::from_str(&json).unwrap();
        assert_eq!(collection, back);
    }

    #[test]
    fn json_uses_camel_case_keys() {
        let json = serde_json::to_string(&sample_collection()).unwrap();
        assert!(!json.contains("\"add_to\"") && json.contains("\"addTo\":\"query\""));
        assert!(!json.contains("\"file_path\"") && json.contains("\"filePath\""));
        assert!(json.contains("\"preRequest\""));
        assert!(json.contains("\"postResponse\""));
    }

    #[test]
    fn every_model_type_roundtrips() {
        // Environment
        let env = Environment {
            id: "e1".to_string(),
            name: "Dev".to_string(),
            values: vec![KeyValue::new("host", "localhost:8080")],
        };
        let back: Environment =
            serde_json::from_str(&serde_json::to_string(&env).unwrap()).unwrap();
        assert_eq!(env, back);

        // AuthKind variants
        for kind in [
            AuthKind::None,
            AuthKind::Bearer {
                token: "t".to_string(),
            },
            AuthKind::Basic {
                username: "u".to_string(),
                password: "p".to_string(),
            },
            AuthKind::ApiKey {
                key: "k".to_string(),
                value: "v".to_string(),
                add_to: ApiKeyIn::Header,
            },
        ] {
            let back: AuthKind =
                serde_json::from_str(&serde_json::to_string(&kind).unwrap()).unwrap();
            assert_eq!(kind, back);
        }

        // HistoryEntry + ResponseSummary (nested inside)
        let entry = HistoryEntry {
            id: "h1".to_string(),
            time: "2026-10-20T10:00:00Z".to_string(),
            request: request("r1", "list", HttpMethod::Get, None),
            response: Some(ResponseSummary {
                status: 200,
                reason: "OK".to_string(),
                size: 1234,
                duration_ms: 42,
                time: "2026-10-20T10:00:01Z".to_string(),
            }),
        };
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"durationMs\""));
        let back: HistoryEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);

        // Empty history entry (response None)
        let entry = HistoryEntry {
            id: "h2".to_string(),
            time: "2026-10-20T10:00:00Z".to_string(),
            request: request("r2", "x", HttpMethod::Post, None),
            response: None,
        };
        let back: HistoryEntry =
            serde_json::from_str(&serde_json::to_string(&entry).unwrap()).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn tree_find_and_mutate() {
        let mut collection = sample_collection();

        // find_request / find_request_mut reach nested requests
        assert_eq!(collection.find_request("r4").unwrap().name, "upload");
        assert!(collection.find_request("nope").is_none());
        collection.find_request_mut("r4").unwrap().url = "changed".to_string();
        assert_eq!(collection.find_request("r4").unwrap().url, "changed");

        // find_item_mut + rename works on both kinds
        collection
            .find_item_mut("f1")
            .unwrap()
            .rename("Renamed".to_string());
        assert_eq!(collection.find_item_mut("f1").unwrap().name(), "Renamed");
        collection
            .find_item_mut("r1")
            .unwrap()
            .rename("renamed req".to_string());
        assert_eq!(collection.find_request("r1").unwrap().name, "renamed req");
    }

    #[test]
    fn insert_into_root_and_folder() {
        let mut collection = sample_collection();
        let before_root = collection.items.len();
        assert!(collection.insert_item(
            None,
            CollectionItem::Request(request("new1", "n", HttpMethod::Get, None))
        ));
        assert_eq!(collection.items.len(), before_root + 1);
        assert!(collection.find_request("new1").is_some());

        let before_folder = collection.find_folder_mut("f1").unwrap().items.len();
        assert!(collection.insert_item(
            Some("f1"),
            CollectionItem::Request(request("new2", "n", HttpMethod::Get, None))
        ));
        assert_eq!(
            collection.find_folder_mut("f1").unwrap().items.len(),
            before_folder + 1
        );
        assert!(collection.find_request("new2").is_some());

        // Unknown parent: nothing inserted, returns false
        assert!(!collection.insert_item(
            Some("missing"),
            CollectionItem::Request(request("new3", "n", HttpMethod::Get, None))
        ));
        assert!(collection.find_request("new3").is_none());
    }

    #[test]
    fn remove_from_root_and_nested_folder() {
        let mut collection = sample_collection();
        assert_eq!(collection.count_requests(), 7);

        // Remove a request nested two folders deep
        let removed = collection.remove_item("r4").unwrap();
        assert_eq!(removed.id(), "r4");
        assert!(collection.find_request("r4").is_none());
        assert_eq!(collection.count_requests(), 6);

        // Remove a whole folder (with its contents)
        let removed = collection.remove_item("f1").unwrap();
        assert_eq!(removed.id(), "f1");
        assert!(collection.find_request("r2").is_none());
        assert_eq!(collection.count_requests(), 4);

        assert!(collection.remove_item("missing").is_none());
        assert_eq!(collection.count_requests(), 4);
    }

    #[test]
    fn count_requests_recursive() {
        let collection = sample_collection();
        assert_eq!(collection.count_requests(), 7);
    }

    #[test]
    fn auth_inheritance_request_beats_folder_and_collection() {
        let collection = sample_collection();
        // r2 has its own ApiKey auth inside f1 (Basic) inside collection (Bearer)
        assert_eq!(
            collection.resolve_request_auth("r2"),
            AuthKind::ApiKey {
                key: "X-Api-Key".to_string(),
                value: "secret".to_string(),
                add_to: ApiKeyIn::Query
            }
        );
    }

    #[test]
    fn auth_inheritance_folder_beats_collection() {
        let collection = sample_collection();
        // r3 has no own auth, sits in f1 (Basic) -> folder wins over collection Bearer
        assert_eq!(
            collection.resolve_request_auth("r3"),
            AuthKind::Basic {
                username: "user".to_string(),
                password: "pass".to_string()
            }
        );
        // r4 sits in f2 (no auth) inside f1 (Basic): innermost defined is f1
        assert_eq!(
            collection.resolve_request_auth("r4"),
            AuthKind::Basic {
                username: "user".to_string(),
                password: "pass".to_string()
            }
        );
    }

    #[test]
    fn auth_inheritance_collection_and_none() {
        let collection = sample_collection();
        // r1 is at root with no auth: collection Bearer applies
        assert_eq!(
            collection.resolve_request_auth("r1"),
            AuthKind::Bearer {
                token: "collection-token".to_string()
            }
        );
        // Unknown id resolves to None
        assert_eq!(collection.resolve_request_auth("missing"), AuthKind::None);

        // Nothing defined anywhere -> AuthKind::None
        let mut bare = sample_collection();
        bare.auth = None;
        assert_eq!(bare.resolve_request_auth("r1"), AuthKind::None);
        assert_eq!(
            bare.resolve_request_auth("r3"),
            AuthKind::Basic {
                username: "user".to_string(),
                password: "pass".to_string()
            }
        );
    }

    #[test]
    fn auth_explicit_none_overrides_inheritance() {
        let mut collection = sample_collection();
        // Explicit AuthKind::None on the request suppresses folder/collection auth.
        collection.find_request_mut("r3").unwrap().auth = Some(AuthConfig {
            kind: AuthKind::None,
        });
        assert_eq!(collection.resolve_request_auth("r3"), AuthKind::None);
    }

    #[test]
    fn helpers_generate_ids_and_times() {
        let a = new_id();
        let b = new_id();
        assert_ne!(a, b);
        assert!(Uuid::parse_str(&a).is_ok());
        assert!(now_rfc3339().ends_with('Z'));
    }
}
