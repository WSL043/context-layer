# Event-bound text content reads

The content-addressed vault is storage, not an authorization namespace. Knowing or guessing a SHA-256 digest is never sufficient to read a blob.

A Local API text read is therefore bound to two pieces of evidence: the canonical `event_id` and one `sha256` content reference carried by that exact event. The server applies authorization in this order:

1. validate the read token and require that the server profile explicitly permits raw text bytes;
2. load the canonical raw row by `event_id`;
3. reject unauthorized scopes before decoding indexed sensitivity or the raw envelope;
4. apply the server-side event-sensitivity grant;
5. decode the envelope and require its event id, scope, and sensitivity to match the indexed raw row;
6. require that this exact event references the requested digest;
7. apply the content reference retrieval class and reject `Secret`;
8. require uncompressed UTF-8 `text/plain` in `local_vault`;
9. perform a bounded vault read, reject symlinks/non-regular files, re-hash the bytes, and require the actual length to match the immutable content reference.

Unknown event IDs, wrong digests, digests referenced only by another event, unauthorized scopes, over-grant event sensitivity, malformed digest syntax, and `Secret` refs all return the same not-authorized result. This avoids using the API as an event/blob existence oracle.

Authorized content that is missing, corrupt, tampered with, or fails its stored length/hash invariant is reported as unavailable only after authorization has succeeded.

## Profile boundary

`metadata` remains metadata-only. It can query the metadata timeline according to its existing grant but cannot read vault bytes, including content references classified as `Normal`.

`sensitive` explicitly enables text-content reads, still subject to the event/scope/reference checks above. There is no `Secret` content-read profile.

## Transport boundary

The first content read is intentionally narrow: one complete UTF-8 `text/plain` object up to 96 KiB. Images, arbitrary binary blobs, compressed content, and larger text are not returned by this command. Larger text should use a future chunk protocol with explicit UTF-8 boundaries and cursor semantics rather than weakening the existing 1 MiB Local API frame limit.

The wire request contains only the bearer token, `event_id`, and `sha256`. The client cannot supply a scope, sensitivity class, retrieval class, or other grant parameter.
