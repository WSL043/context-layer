# Retrieval v1 boundary

Personal Context capture and Personal Context retrieval are separate trust decisions.

Retrieval v1 establishes an internal, policy-gated timeline query before any broad read command is added to the local IPC surface. A caller receives a `RetrievalGrant` from trusted composition code; the query itself cannot request or upgrade its own grant.

The first query surface is intentionally narrow:

- one explicit scope per query;
- optional half-open time range `[start_at, end_at)`;
- descending keyset pagination using `(observed_at, event_id)` rather than offsets;
- bounded pages (maximum 200 visible events) and bounded raw scanning per request;
- event sensitivity filtering before an event becomes visible;
- content references filtered independently by retrieval class;
- payload returned only when the grant permits payloads and every referenced content object is visible under that grant;
- raw vault bytes are not returned by this layer.

A metadata-only grant is the conservative baseline: it can see only `Metadata` events, only `Normal` content-reference metadata, and never payloads. Sensitive events are not returned as redacted placeholders because even revealing their existence/timing can itself be sensitive.

The storage layer performs only stable raw-row scanning. Policy lives in the core retrieval engine so SQLite, future import stores, and other backends cannot drift into different authorization semantics.

The first database change is only a versioned index on `(scope_id, observed_at, event_id)` to support stable keyset scans. No semantic projection or denormalized personal-data index is introduced yet.

This slice deliberately does **not** expose timeline querying through `LocalApiCommand`. Before IPC reads are enabled, the caller/capability identity and grant assignment need an explicit design so an arbitrary same-user process cannot simply ask the agent for all Sensitive evidence.
