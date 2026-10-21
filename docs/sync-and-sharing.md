# Sync and sharing

restrust separates local work from server-backed collaboration: desktop request execution and local editing work with no account or server, and an optional self-hosted server adds synchronization, sharing with users and organizations, and public publishing. This page explains namespaces, what happens offline, how conflicting edits are resolved, the permission model, and exactly what revocation does and does not do.

## Independent local operation

The desktop client is a complete tool on its own: install it and start working without registration or a server. Web clients require a signed-in server connection for request execution. Collections, folders, requests, environments, variables, and history are all persisted locally (a JSON workspace file on desktop, browser local storage on web) and survive restarts. Disconnecting from a server preserves your local data.

## Namespaces

Your work is organized into two kinds of namespace, switchable in the sidebar:

- **Personal** — your own space. Personal collections are local by default; they are only stored on a server if you explicitly enable synchronization for them.
- **Organizations** — server-side spaces owned by an organization, visible to you when you are a member. Organization collections live on the server, but any collection you have synchronized remains locally readable and editable while offline (see below).

Organization membership carries a role per organization (managed in the [Administration](administration.md) menu): **viewer** can read, **editor** can create and edit organization collections, and **admin** can additionally manage and delete them.

## Optional synchronization

Signing in to a server does not silently upload your personal work:

- Personal collections stay local until you choose **Enable sync** on them. After that, edits are pushed to the server and the collection keeps a server revision.
- Organization collections you can see are synced as part of your membership.
- Desktop clients can join multiple servers using the server menu beside the app menu. Choose **Join another server…**, sign in, and switch between connections without signing out of the others. The selected server controls synchronization, organization membership, sharing, and administration. Local collections remain available across servers.
- **Remember this server** saves its address. Tokens remain in memory and must be renewed by signing in after restarting the client. Signing out affects only the selected connection. **Local workspace** returns to local collections while retaining the other sessions.
- Web clients use the server on their current origin. Browser sends always execute there; desktop sends execute directly from the desktop.

## Offline editing and resynchronization

A synchronized collection is fully usable while the server is unreachable: you can read it, edit it, add requests, and send requests from desktop. Browser sends require the execution server to be reachable. Edits made offline are marked as pending and kept locally. When the server becomes reachable again, choose **Sync Now** to push pending edits. If the server stays unreachable, nothing is lost and nothing is deleted — your edits remain local and sync when possible.

## Conflict policy

Synchronization is revision-based. Every collection on the server carries a monotonically increasing revision number, and a push carries the revision it was based on. When two people (or two of your devices) edited the same collection concurrently, the server rejects the stale push with a conflict, and the client asks you to choose:

- **Overwrite server** — force your local version onto the server, replacing what was pushed in the meantime.
- **Keep local copy** — leave the server copy as is and retain your version locally.

One caveat in the current version: deleting a collection on the server does not propagate the deletion into existing local copies; the collection simply stops appearing in the server listing. Also note that, unlike the requests inside a collection, nothing else is merged: sync replaces content at collection granularity.

## Sharing personal collections

The owner of a personal collection can share it from the collection's context menu with:

- **selected users**, at one of two levels:
  - **Read only** — the recipient can view and use the collection, but changes are not saved to the server.
  - **Full Access** — the recipient can create, edit, and delete requests and folders inside the collection.
- **the entire organization** — membership then controls access (using the roles above).

Some actions stay reserved for the owner, regardless of who has Full Access:

- deleting the collection itself,
- managing its sharing settings,
- publishing or unpublishing it.

A Full Access grant is a content-management grant, not a transfer of ownership.

## Public publishing

The owner of a collection can publish it with a slug, producing a stable public URL on the server (`/api/v1/public/{slug}`) that any visitor can open in a browser without signing in. Published content is read-only: visitors get a rendering of the collection — its tree of requests with method, URL, and description — plus a JSON representation for programmatic use. Publishing never grants editing rights, and the owner can unpublish at any time to remove the public URL.

## Revocation

When a share is removed, your organization membership ends, or the owner otherwise withdraws access, the server denies further synchronization of the affected collection. In the client:

- The collection appears **grayed out and disabled** in the tree, with a *revoked* badge.
- Opening it shows a banner stating that access was revoked and synchronization is disabled.
- You choose what happens to the retained local copy — nothing is deleted automatically:
  - **Delete local data** — removes your local copy after an explicit confirmation.
  - **Save as personal local copy** — moves the content into your Personal namespace as an independent local collection, with no link to the original and no further synchronization to it.

Two guardrails hold by design. First, only a confirmed access-denied response from the server triggers this flow: a transient network error, a timeout, or the server being down is never interpreted as revocation, and no destructive option is offered for those. Second, revocation removes access to the shared resource going forward; it does not reach into your machine to erase data you already received — which is why the choice above is yours.
