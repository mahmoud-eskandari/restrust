# restrust documentation

restrust is a cross-platform (Windows, macOS, Linux, and the browser) Rust API client for designing, organizing, executing, testing, and sharing APIs. The desktop app operates fully independently on your machine — no account and no server required — and offers an optional self-hosted server for synchronization, team collaboration, and public publishing.

## Documentation index

| Page | Contents |
| --- | --- |
| [Getting started](getting-started.md) | Prerequisites, building and running the desktop and browser clients, a server quickstart, and your first request. |
| [Configuration](configuration.md) | All server environment variables, the precedence rule, TLS modes (none, files, ACME), and sign-in provider setup. |
| [Sync and sharing](sync-and-sharing.md) | Namespaces, optional synchronization, offline editing, conflict handling, sharing, public publishing, and revocation behavior. |
| [Import, export, and snippets](interchange.md) | Postman, OpenAPI, and cURL interchange with explicit fidelity notes, plus code snippet generation. |
| [Administration](administration.md) | The role-gated administration menu: user management, organizations, and sign-in options. |
| [Architecture](architecture.md) | Workspace layout, crate responsibilities, client platform services, persistence formats, and security notes. |

## Where to start

- If you want to send an API request within minutes, read [Getting started](getting-started.md). Desktop execution needs no account or server; browser execution uses a signed-in restrust server.
- If you are deploying the optional server for a team, read [Configuration](configuration.md) and [Sync and sharing](sync-and-sharing.md).
- If you are migrating from another tool, read [Import, export, and snippets](interchange.md).

## Support and development

The project is developed in the open on GitHub, where you can report issues and contribute changes. See the repository root [README](../README.md) for build instructions, the development workflow (formatting, linting, tests), and continuous-integration status.
