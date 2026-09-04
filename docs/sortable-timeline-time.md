# Sortable timeline time keys

`raw_event.observed_at` is preserved as original RFC3339 evidence, but RFC3339 text is not a safe SQLite ordering key when fractional seconds are optional. A whole-second value such as `2023-11-14T22:13:20Z` sorts lexicographically after `2023-11-14T22:13:20.5Z`, even though it is the earlier instant.

Context Layer therefore keeps two representations with different jobs:

- `observed_at`: original timestamp evidence, returned to clients and never rewritten for indexing convenience;
- `observed_key`: derived, rebuildable UTC sort state formatted as fixed-width `YYYY-MM-DDTHH:MM:SS.nnnnnnnnnZ`.

Timeline range predicates, descending order, and keyset pagination use `observed_key` plus `event_id` as the tie-breaker. The original `observed_at` remains the value exposed in raw/timeline records.

SQLite schema v4 backfills `observed_key` for every existing raw event by parsing its stored RFC3339 timestamp, drops the earlier text-time index, and creates `raw_event_scope_observed_key_cursor` on `(scope_id, observed_key DESC, event_id DESC)`. New v1 and v2 raw events write the derived key atomically with the event, and a trigger rejects future inserts that omit it.

The sortable representation intentionally supports UTC years 0000–9999. Values outside that range are rejected at the storage boundary rather than silently producing keys whose lexical ordering contract is undefined.
