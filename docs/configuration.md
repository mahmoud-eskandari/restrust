# Server configuration

The restrust server is configured entirely through `RESTRUST_*` environment variables, with persisted settings in its SQLite database as a fallback. This page lists every variable, explains the precedence rule, and gives guidance for the three TLS modes (including ACME certificate issuance and its requirements) and for enabling Google or generic SSO sign-in.

## Precedence

For each setting, the applied value is resolved in this order:

1. **Environment variable** (`RESTRUST_*`), if set.
2. **SQLite `settings` table**, if the key is persisted there.
3. **Built-in default** listed in the table below.

In short: env var > SQLite settings table > built-in default. The applied configuration is logged at startup, and `GET /api/v1/healthz` reports the effective non-secret settings, so you can verify what a running server resolved.

## Environment variables

| Env | Default | Meaning |
|---|---|---|
| RESTRUST_BIND_ADDR | 0.0.0.0 | listen address |
| RESTRUST_HTTP_PORT | 8080 | HTTP port (ACME HTTP-01 + app) |
| RESTRUST_HTTPS_ENABLED | false | enable TLS listener |
| RESTRUST_HTTPS_PORT | 8443 | HTTPS port |
| RESTRUST_DOMAIN | (none) | public domain; required for ACME |
| RESTRUST_TLS_MODE | none | `acme` \| `files` \| `none` |
| RESTRUST_TLS_CERT_FILE / RESTRUST_TLS_KEY_FILE | - | operator-supplied cert (mode=files) |
| RESTRUST_ACME_DIRECTORY | production | `production` \| `staging` or full URL |
| RESTRUST_ACME_CONTACT_EMAIL | - | ACME account email |
| RESTRUST_DATA_DIR | ./data | root for db + files + acme state |
| RESTRUST_DB_PATH | {DATA_DIR}/restrust.db | SQLite file |
| RESTRUST_FILES_DIR | {DATA_DIR}/files | uploaded/exported files |
| RESTRUST_WEB_DIR | (none) | static web client dir served at `/` (optional) |
| RESTRUST_PUBLIC_BASE_URL | derived from domain/ports | absolute base used in links |
| RESTRUST_BOOTSTRAP_ADMIN_USER / RESTRUST_BOOTSTRAP_ADMIN_PASSWORD | - | create sysadmin if no admin exists (idempotent) |
| RESTRUST_GOOGLE_CLIENT_ID / RESTRUST_GOOGLE_CLIENT_SECRET | - | enable Google OIDC sign-in |
| RESTRUST_SSO_{i}_NAME / _ISSUER / _CLIENT_ID / _CLIENT_SECRET (i=1..5) | - | generic OIDC providers (discovery via `{issuer}/.well-known/openid-configuration`) |

Paths in defaults (`{DATA_DIR}`) expand to the resolved `RESTRUST_DATA_DIR` value.

## Common configurations

A local development server needs no configuration at all:

```sh
cargo run -p restrust-server --release
```

A typical production deployment serving HTTPS directly (no reverse proxy required):

```sh
RESTRUST_DOMAIN=api.example.com \
RESTRUST_TLS_MODE=acme \
RESTRUST_HTTPS_ENABLED=true \
RESTRUST_HTTP_PORT=80 \
RESTRUST_HTTPS_PORT=443 \
RESTRUST_ACME_CONTACT_EMAIL=admin@example.com \
RESTRUST_BOOTSTRAP_ADMIN_USER=admin \
RESTRUST_BOOTSTRAP_ADMIN_PASSWORD=change-me \
restrust-server
```

With `RESTRUST_WEB_DIR=/path/to/client/dist` added, the server also hosts the browser client at `/` (with a fallback to `index.html` for hash-routed pages); API endpoints always take precedence over static files.

## TLS modes

`RESTRUST_TLS_MODE` selects one of three modes. With TLS disabled, the server runs a single HTTP listener on `RESTRUST_HTTP_PORT` (the development default).

| Mode | When to use |
| --- | --- |
| `none` | Local development, testing, or a trusted internal network where TLS is not required. |
| `files` | You already hold a certificate (for example from your organization's certificate authority) or the server runs on a network isolated from the public internet. Set `RESTRUST_TLS_CERT_FILE` and `RESTRUST_TLS_KEY_FILE` (PEM format) plus `RESTRUST_HTTPS_ENABLED=true`. |
| `acme` | The server is reachable from the public internet on a domain you control. The server obtains and renews Let's Encrypt certificates by itself; no external TLS helper or reverse proxy is needed. |

### ACME requirements

ACME mode uses the HTTP-01 challenge, so the certificate authority must be able to reach your server:

- `RESTRUST_DOMAIN` must be set to the public domain the certificate is issued for, and the domain must resolve to the server.
- Ports 80 and 443 must be reachable from the internet: set `RESTRUST_HTTP_PORT=80` and `RESTRUST_HTTPS_PORT=443`. The HTTP listener answers ACME challenges and redirects other traffic to HTTPS.
- `RESTRUST_HTTPS_ENABLED=true` enables the TLS listener.
- ACME account state and issued certificates are cached under `RESTRUST_DATA_DIR` (`acme/`), so they survive restarts and satisfy certificate-authority rate expectations.

### Isolated networks

A server on a fully isolated network cannot complete public certificate-authority validation: the CA has no way to reach it to prove domain control. Such deployments should use `files` mode with operator-supplied certificates — typically issued by an internal certificate authority that all clients of that network trust. restrust cannot fabricate publicly trusted certificates for unreachable hosts.

### Staging versus production

Set `RESTRUST_ACME_DIRECTORY=staging` while setting up or testing ACME: the Let's Encrypt staging directory issues untrusted-but-structurally-valid certificates with generous rate limits, so a broken configuration does not burn your production issuance quota. Switch to `production` (the default) once the flow works. A full directory URL is also accepted for other ACME providers.

## Sign-in providers

Local username/password accounts always work. Google and generic SSO providers are OpenID Connect (OIDC) and are enabled purely by configuration; users are provisioned automatically on first sign-in.

### Google

1. Create an OAuth 2.0 client in the Google Cloud Console for your server deployment.
2. Register the redirect URI `{base}/api/v1/auth/oidc/google/callback`, where `{base}` is your server's public base URL — for example `https://api.example.com/api/v1/auth/oidc/google/callback`.
3. Set `RESTRUST_GOOGLE_CLIENT_ID` and `RESTRUST_GOOGLE_CLIENT_SECRET`.

Once both variables are set, the client's sign-in dialog offers Google as a provider. On the desktop this opens your browser and returns a copyable session token to paste back; when the web client is hosted by the same server, the token is applied automatically.

### Generic SSO (OIDC)

Up to five additional providers are supported via indexed variables, where `{i}` runs from 1 to 5:

```sh
RESTRUST_SSO_1_NAME="Corp SSO"
RESTRUST_SSO_1_ISSUER=https://sso.corp.example.com
RESTRUST_SSO_1_CLIENT_ID=...
RESTRUST_SSO_1_CLIENT_SECRET=...
```

The server discovers the provider's endpoints automatically from `{issuer}/.well-known/openid-configuration`. Register the redirect URI `{base}/api/v1/auth/oidc/sso_{i}/callback` (for example `.../sso_1/callback` for `i=1`) with the provider. The provider appears in the sign-in dialog under the configured name.

## Bootstrap administrator

`RESTRUST_BOOTSTRAP_ADMIN_USER` and `RESTRUST_BOOTSTRAP_ADMIN_PASSWORD` create a system administrator account at startup **only if the server has no admin yet**. The check runs on every start, so the variables are safe to leave in place; once an admin exists they do nothing. Use them for the first deployment, then manage users through the client's [Administration](administration.md) menu. See [Administration](administration.md) for what the role can do.
