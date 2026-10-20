# Administration

Server administration happens inside the restrust client: users with the server's admin role get an **Administration** menu that provides user management, organization management, and an overview of the server's sign-in options. This page describes what the menu contains, and the permission model that stands behind it — notably that the server, not the client UI, decides what an administrator may do.

## The Administration menu and role gating

The Administration menu appears in the client only when you are signed in with an account whose system role is **admin**. For everyone else it is hidden, and the client behaves exactly like a standard user installation.

Menu visibility is a convenience, never the security boundary. Every administrative action calls a server API, and the server re-checks the caller's current role on **every** privileged call. Consequences worth knowing:

- If your role is removed server-side, the actions simply start failing with an access-denied error, even if a stale client still shows the menu.
- Calling the admin APIs directly (bypassing the client) grants nothing without the admin role.
- When the server rejects an action, the client shows the server's error message verbatim rather than guessing.

## User management

The **Users** tab lists every account on the server with its username, display name, system role, disabled state, and creation date. From here an administrator can:

- **create a user** — username, initial password, and system role (`user` or `admin`);
- **disable or re-enable an account** — a disabled account cannot sign in; its data stays intact;
- **change the system role** of an account;
- **set a new password** for an account (for example on recovery).

The first administrator is not created in the UI — a fresh server has nobody to log in as. It is created by the `RESTRUST_BOOTSTRAP_ADMIN_USER` / `RESTRUST_BOOTSTRAP_ADMIN_PASSWORD` environment variables at startup, which act only while the server has no admin at all. See [Configuration](configuration.md#bootstrap-administrator).

## Organizations

The **Organizations** tab manages the organization namespaces used for team collaboration (see [Sync and sharing](sync-and-sharing.md) for how namespaces behave for end users). An administrator can:

- **list organizations** with their current members and each member's role;
- **create an organization**;
- **replace an organization's member list** — a set of username/role pairs, where the role is `admin`, `editor`, or `viewer` within that organization:
  - **viewer** — read access to the organization's collections,
  - **editor** — additionally create and edit organization collections,
  - **admin** — additionally manage and delete organization collections.

Editing members takes effect immediately for subsequent sign-ins and API calls; a removed member's already-synchronized copies follow the revocation flow described in [Sync and sharing](sync-and-sharing.md#revocation).

## Sign-in options

The administration surface works with whichever sign-in options the server is configured to offer. Three kinds are supported, and they can be combined:

- **Local accounts** — username and password, stored on the server with argon2id password hashing. Always available; created by an administrator or the bootstrap variables.
- **Google** — OpenID Connect sign-in via the Google preset, enabled by setting `RESTRUST_GOOGLE_CLIENT_ID` / `RESTRUST_GOOGLE_CLIENT_SECRET`.
- **SSO (generic OIDC)** — up to five additional OpenID Connect providers configured through the `RESTRUST_SSO_{i}_*` variables, appearing under their configured names.

Provider setup — redirect URIs, issuer discovery, and staging advice — is covered in [Configuration](configuration.md#sign-in-providers). Users signing in through a provider for the first time are provisioned automatically; the account is linked to the provider identity, and matches by email address are linked to an existing account.
