# Import, export, and code snippets

restrust exchanges API definitions with the tools and formats teams already use: Postman collections and environments, OpenAPI documents, and cURL commands, in both directions where meaningful. Every import produces an explicit report of what was preserved, what was converted lossily, and what is unsupported, so nothing disappears silently. This page lists the supported formats, the conversion behavior, and the known fidelity limits — read the fidelity notes before relying on a round trip.

## Supported formats

| Format | Import | Export |
| --- | --- | --- |
| Postman collection v2.1 | Yes | Yes |
| Postman collection v3.0 | Yes (best effort) | — |
| Postman environment JSON | Yes | — |
| OpenAPI 3.0 (JSON or YAML) | Yes | Yes (lossy) |
| OpenAPI 3.1 (JSON or YAML) | Yes | — |
| cURL command | Yes | Yes |

## Postman collections

### Import (v2.1 and v3.0)

Import detects the collection version automatically from the document's schema identifier. Version 2.1 imports with full structural fidelity: folders, requests, query parameters, headers, bodies (raw, URL-encoded, form-data, binary file references), collection variables, and authentication (bearer, basic, API key) all map to their restrust equivalents, and pre-request/post-response scripts are preserved as text (see fidelity notes). Version 3.0 imports on a best-effort basis; constructs that cannot be represented are listed in the import report rather than dropped quietly.

### Export (v2.1)

Export writes a Postman v2.1 collection that re-imports into restrust identically, and opens in Postman tooling. restrust-specific concepts — synchronization state, sharing grants, publication settings, and other server-side metadata — are not part of the Postman formats and are not included; the export contains the collection content itself.

## Postman environments

Postman environment JSON files import as restrust environments (name plus variable values), selectable from the environment dropdown like any other environment.

## OpenAPI documents

### Import (3.0.x and 3.1.x, JSON or YAML)

Each operation becomes a request in the generated collection:

- server URL plus path become the request URL,
- operation parameters become query parameter and header rows (with path-level parameters applied to the operations under them),
- the request body (for JSON media types) is carried over as a raw JSON body,
- security schemes become an authentication placeholder on the request,
- the operation summary and description become the request description.

The import report notes losses, such as schema details, examples, and response definitions that have no restrust equivalent in this direction.

### Export (OpenAPI 3.0)

Export converts requests back into paths and operations — method, path, and basic metadata only — and is explicitly lossy: bodies, parameters, and descriptions are included where they map, but authentication details, restrust variables, and organization are reduced. Prefer Postman v2.1 export when the goal is a faithful round trip of collection content.

## cURL

Paste a `curl` command to import it as a request. The parser understands the common flags, including `-X` (method), `-H` (headers), `-d`, `--data`, and `--data-raw` (body), `-u` (basic auth), `-b` (cookie header), and `--compressed`. In the other direction, any request can be exported or copied as a cURL command from its context menu (**Copy as cURL**).

## Code snippets

Any request can be generated as a ready-to-run snippet for:

- cURL
- Rust (`reqwest`)
- Python (`requests`)
- JavaScript (`fetch`)
- Go (`net/http`)

Snippets are generated locally from the request definition (method, URL, parameters, headers, body, and authentication where the target library has a natural representation). Generation never executes anything.

## Fidelity notes

Read these before migrating work or promising a lossless round trip:

- **Scripts are never executed.** Pre-request and post-response scripts imported from Postman are preserved as text attached to the collection and shown in the UI, but restrust does not execute them. They round-trip through Postman export.
- **Form-data files are preserved as names only.** A multipart body referencing a file keeps the file name across import/export; the file's bytes are not embedded anywhere.
- **Unknown constructs are reported.** Every import produces a report with three lists — **preserved** (kept as-is), **lossy** (converted with a known reduction), and **unsupported** (present in the source, not representable). Check the report after each import.
- **restrust metadata does not travel.** Synchronization state, sharing grants, publication settings, and other restrust-specific metadata are not represented in Postman or OpenAPI formats; exports carry collection content only.
