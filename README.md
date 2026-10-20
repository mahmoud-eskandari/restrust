# restrust

![CI](https://github.com/mahmoud-eskandari/restrust/actions/workflows/ci.yml/badge.svg)

restrust is a cross-platform API client for Windows, macOS, Linux, and the browser, built in Rust. It works fully offline as a personal tool — no account, no sign-in, no server — and the same application connects to an optional self-hosted server for synchronization, team sharing, and public publishing. It exchanges collections with Postman and OpenAPI documents and generates code snippets for common languages.

## Features

- **Independent local operation** — the full client works with no account and no server; connect one later without losing anything.
- **Collections, folders, and requests** — tree navigation with tabs, search, duplication, and history.
- **Environments and variables** — global, environment, collection, and request scopes with `{{variable}}` interpolation.
- **Response inspection** — status, timing, and size at a glance; body (pretty/raw) and header views; copy and save.
- **Interchange** — Postman v2.1 import/export, Postman v3.0 import, Postman environments, OpenAPI 3.0/3.1 import (JSON and YAML), lossy OpenAPI 3.0 export, cURL import/export — each import reports preserved/lossy/unsupported items.
- **Code snippets** — generate cURL, Rust (`reqwest`), Python (`requests`), JavaScript (`fetch`), and Go (`net/http`) from any request.
- **Optional self-hosted sync and sharing** — personal sync on demand, sharing with users (Read only / Full Access) or organizations, offline editing with revision-based conflict resolution, and public read-only publishing at a stable URL.
- **Administration in the client** — a role-gated menu for user management, organizations, and sign-in options; every privileged call is authorized server-side.
- **Direct HTTP/HTTPS server** — terminates TLS itself, including automatic Let's Encrypt (ACME) certificates, with no reverse proxy required.

## Quickstart

```sh
# Desktop client (Windows, macOS, Linux)
cargo run -p restrust-client --release

# Browser client
rustup target add wasm32-unknown-unknown   # once
cargo install --locked trunk               # once
cd client && trunk serve

# Optional self-hosted server (default: http://localhost:8080)
RESTRUST_BOOTSTRAP_ADMIN_USER=admin \
RESTRUST_BOOTSTRAP_ADMIN_PASSWORD=change-me \
cargo run -p restrust-server --release
curl http://localhost:8080/api/v1/healthz
```

## Documentation

- [Documentation index](docs/README.md)
- [Getting started](docs/getting-started.md)
- [Server configuration](docs/configuration.md)
- [Sync and sharing](docs/sync-and-sharing.md)
- [Import, export, and snippets](docs/interchange.md)
- [Administration](docs/administration.md)
- [Architecture](docs/architecture.md)

## Development

```sh
cargo fmt --all --check              # formatting
cargo clippy --workspace --all-targets -- -D warnings   # lint
cargo test --workspace               # tests
```

CI runs formatting, linting, and tests on Windows, macOS, and Linux, checks and builds the browser client for `wasm32-unknown-unknown`, and produces a release server binary — see [`.github/workflows/ci.yml`](.github/workflows/ci.yml).

## License

Dual-licensed under `MIT OR Apache-2.0`, at your option — matching the Rust workspace's license field.
