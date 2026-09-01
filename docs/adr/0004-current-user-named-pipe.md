# ADR 0004: Current-user Named Pipe for local clients

Status: Accepted

## Context

The browser Native Messaging host and future desktop UI need a stable agent API.
Opening a loopback TCP port creates an unnecessary network listener and makes
ownership harder to express. Direct SQLite access would couple every client to
storage migrations and violate the single-writer rule.

## Decision

Windows clients use a byte-mode Named Pipe named with the current user SID. The
server rejects remote clients and supplies a protected DACL granting access only
to LocalSystem and that SID. Messages use a four-byte little-endian length prefix,
UTF-8 JSON, a 1 MiB hard limit, request IDs, and an explicit protocol version.

The DTOs live in `context-contracts`; framing and Windows transport live in
`context-local-ipc`; only `context-agent` opens SQLite. Browser-origin validation
remains the Native Host's responsibility.

## Consequences

- The UI and browser bridge can change independently of SQLite tables.
- A standard-user install needs no service, firewall rule, or administrator ACL.
- Same-user malware remains outside the threat model and can still impersonate a
  client; release signing and process integrity are separate controls.
- macOS and Linux will implement their native current-user local transports behind
  the same DTO and framing contract rather than reusing Windows-specific code.
