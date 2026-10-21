# Getting started

restrust runs as a desktop application on Windows, macOS, and Linux, and as a browser application built with WebAssembly. This page takes you from a fresh checkout to a sent API request on the desktop or in the browser, and through a minimal server setup for when you want synchronization and sharing. The desktop client sends requests without an account or server. The browser client can edit collections offline, but sending requests requires signing in to the server that hosts it.

## Prerequisites

- **Rust**, stable toolchain, installed with [rustup](https://rustup.rs/). The repository pins the toolchain channel in `rust-toolchain.toml`, so rustup selects it automatically inside the checkout. On Windows, follow the rustup/MSVC linker setup for your system.
- **For the browser client only:** the `wasm32-unknown-unknown` target and [Trunk](https://trunkrs.dev/):

  ```sh
  rustup target add wasm32-unknown-unknown
  cargo install --locked trunk
  ```

- **For the server:** no extra prerequisites. The server is a plain Rust binary; its SQLite engine is compiled in.

## Build and run the desktop client

From the repository root:

```sh
cargo run -p restrust-client --release
```

The first release build takes a while; drop `--release` for a faster debug build while iterating. The application starts in local mode with no sign-in screen, and your workspace is persisted to disk automatically (see [Architecture](architecture.md) for storage locations).

## Run the browser client

From the `client/` directory:

```sh
cd client
trunk serve
```

The checked-in Trunk configuration serves the client at `http://localhost:8081` and proxies `/api/` to the restrust server at `http://127.0.0.1:8080`. Start the server below, then sign in from the web client. This keeps browser requests on the same origin.

The browser client persists your workspace in local storage, keyed per origin. Send calls the authenticated `/api/v1/execute` endpoint; the server makes the target HTTP request and returns its status, headers, body, and timing. Target APIs do not need CORS headers. Requests have a 30-second deadline and a 10 MiB body limit in each direction. Targets such as `localhost` refer to the server machine.

For deployment, build with `cd client && trunk build --release`, then run the server with `RESTRUST_WEB_DIR` pointing to the absolute path of `client/dist`. Serve the UI and API together.

## Quickstart: the server

The server adds accounts, synchronization, sharing, and public publishing. To try it locally:

```sh
RESTRUST_BOOTSTRAP_ADMIN_USER=admin \
RESTRUST_BOOTSTRAP_ADMIN_PASSWORD=change-me \
cargo run -p restrust-server --release
```

The server listens on `http://localhost:8080` by default and stores its data under `./data`. Verify it is up:

```sh
curl http://localhost:8080/api/v1/healthz
```

The two `RESTRUST_BOOTSTRAP_ADMIN_*` variables create the first administrator account if the server has none yet; the step is idempotent, and the credentials are only used when no admin exists. See [Configuration](configuration.md) for the full list of environment variables (ports, TLS, storage paths, sign-in providers) and their precedence.

To connect the client to the server, open the client's account menu, choose **Sign in**, enter the server URL and your username and password, and sign in. After signing in, personal collections can be synchronized on demand, and collections shared with you or owned by your organizations appear under their namespaces. Desktop request execution and local editing continue to work without a server.

## First steps in the client

These steps need no account on desktop. In the browser, sign in before sending:

1. **Create a collection.** In the sidebar, use the new-collection action, give it a name, and it appears in your Personal namespace.
2. **Add a request.** Open the collection's context menu and choose **New Request** (a folder is available for grouping).
3. **Fill in and send the request.** Pick the HTTP method, enter the URL, add query parameters, headers, a body, or authentication as needed, then press **Send** (`Ctrl+Enter`). The response viewer shows the status, timing, size, headers, and body.
4. **Reuse it later.** Variables (global, environment, or collection scope) can be referenced as `{{variable}}` in URLs, parameters, headers, and bodies. Every sent request is recorded in History at the bottom of the sidebar.

Importing existing work (Postman collections and environments, OpenAPI documents, cURL commands) is covered in [Import, export, and snippets](interchange.md).

## Where to go next

- [Configuration](configuration.md) — deploying the server properly: TLS, domains, storage, sign-in providers.
- [Sync and sharing](sync-and-sharing.md) — namespaces, offline behavior, conflicts, and permissions.
- [Architecture](architecture.md) — how the pieces fit together.
