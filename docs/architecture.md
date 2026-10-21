# Architecture

restrust is a Rust workspace with one shared domain crate, one interchange crate, a cross-platform client, and a standalone server. The design goal behind the split is that the domain model and all logic stay independent of UI, async runtime, and operating system, so the same core code runs in the desktop client, the browser client, and the server. This page documents the layout, each crate's responsibility, the platform-service boundary in the client, the persistence formats, and the security-relevant implementation choices.

## Workspace layout

```
Cargo.toml                 workspace root; pins shared dependency versions
crates/restrust-core/      domain model and logic
crates/restrust-interchange/ Postman / OpenAPI / cURL import+export, code snippets
client/                    egui/eframe application (desktop native + wasm32)
server/                    axum HTTP server (binary: restrust-server)
docs/                      end-user documentation
```

| Crate/package | Depends on | Responsibility |
| --- | --- | --- |
| `restrust-core` | (none beyond serde-style utilities) | The domain model — collections, folders, requests, authentication, bodies, environments, history, sharing, and workspace documents — plus variable interpolation and sync-state types. Contains no UI, no async runtime, no OS APIs, and compiles for both native and `wasm32-unknown-unknown`. |
| `restrust-interchange` | `restrust-core` | All conversion in and out of foreign formats: Postman v2.1/v3.0 collections, Postman environments, OpenAPI 3.0/3.1, cURL import/export, and code-snippet generation. Produces a structured report of preserved/lossy/unsupported items. |
| `restrust-client` (in `client/`) | core, interchange | The egui/eframe application. One codebase, two entries: native desktop via `main.rs`, and WebAssembly via the `wasm-bindgen` start hook. Owns all UI and platform integration. |
| `restrust-server` (in `server/`) | core | The self-hosted axum server: authentication, sessions, sync API, sharing, publication, admin API, and optional static hosting of the web client. |

The server deliberately does not depend on the interchange crate: it stores and serves collections in the core model's JSON form, and all format conversion happens client-side.

## Client platform services

The client UI never touches the file system, network, clipboard, or browser directly. Each capability sits behind a small trait with two implementations, selected at compile time:

| Service | Native implementation | WebAssembly implementation |
| --- | --- | --- |
| `StorageService` | JSON file at `<OS data dir>/restrust/workspace.json`, written atomically (temp file + rename) on a background thread; an unparseable file is kept as `.bak` before being replaced. | Browser `localStorage`, key `restrust.workspace.v1`. |
| `ClipboardService` | Through the egui context. | Through the egui context (same code path). |
| `HttpService` | `ehttp` async fetch; completion triggers a UI repaint via a channel. The UI thread never blocks on I/O. | Browser `fetch` calls the same-origin restrust API. Target requests are encoded for `/api/v1/execute` and executed by the server using `reqwest`. |
| `BrowserService` | Opens URLs with the OS handler. | Opens URLs in a new browser tab. |

Two consequences of this design: request sending and workspace saving are asynchronous by construction, and the workspace document is the single source of persisted truth on both platforms (see below).

## Persistence formats

### Client workspace document

The client persists everything as one JSON workspace document (with a `format_version` field, currently `1`):

- personal and organization-side stored collections — each carrying the collection content plus sync metadata: sync state (`Local`, `Synced` with revision, `PendingEdits` with base revision, or `Revoked`), server origin (when synced), and the caller's access level;
- global variables, environments, request history, and client settings (remembered server URLs, last selected server, active environment, theme). Server tokens and per-server session/access caches stay in memory.

Desktop stores this file under the OS data directory; the browser build stores it in `localStorage`. Corrupt or unparsable state is backed up, not silently discarded, and the client starts from a fresh workspace rather than crashing.

### Server persistence

The server keeps two kinds of durable state under its data directory:

- a **SQLite database** (WAL mode, migrations applied at startup) holding users, sessions, the `settings` table (see [Configuration](configuration.md) precedence), organizations and memberships, collections with their revision counters, share grants, and publication records;
- a **files directory** for file data, plus ACME account/certificate state when TLS-ACME is in use.

Backups must cover both the database file and the files directory together.

## Sync and revision model

Synchronization is revision-based at collection granularity. Each server-side collection has a monotonically increasing integer revision. A push sends the base revision the client built on; the server accepts only if it matches its current revision, otherwise it answers with a conflict and the client offers overwrite-server or keep-local (user-facing behavior: [Sync and sharing](sync-and-sharing.md)). The client tracks each collection's state (`Local`, `Synced`, `PendingEdits`, `Revoked`) and transitions between them in a pure logic module that is unit-testable without any UI. Server deletion of a collection does not propagate into local copies in the current version.

## Security notes

- **Passwords** are hashed with argon2id; the server never stores plaintext credentials. Login is username/password against a local account or OIDC through a configured provider.
- **Sessions** use 256-bit random tokens accepted as `Authorization: Bearer` headers, and additionally as an `rt_session` cookie so the same-origin web client works without token plumbing. Sessions expire after 30 days.
- **OIDC** (Google preset and generic providers) uses the server-side authorization-code flow; ID tokens are verified via the provider's published JWKS (RS256 accepted). First sign-in provisions an account; accounts are linked by verified email match.
- **TLS** is terminated by the server itself — no reverse proxy is required — in one of three modes (`none`, operator-supplied certificate `files`, or automatic ACME); see [Configuration](configuration.md#tls-modes).
- **Authorization** is enforced server-side on every privileged call; client-side role gating of the Administration menu is UI convenience only (see [Administration](administration.md)).

## Web request execution

`POST /api/v1/execute` requires a valid session. The resolved target method, URL, headers, and bytes are carried in JSON; the server returns a JSON response envelope so target HTTP errors and cookies remain data. The restrust session credential is never forwarded to the target. Only HTTP/HTTPS targets are accepted, framing headers are generated by the outbound client, redirects are limited to 10, the exchange has a 30-second timeout, and request/response bodies are limited to 10 MiB. Authenticated users can reach endpoints accessible from the server, including private services.

Desktop server connections retain independent sessions, roles, sharing caches, and namespaces. Sync filters collections by their server origin; local collection IDs are distinct from server IDs and are translated at the API boundary.
