# Local read capability v1

The local Named Pipe is restricted to the current Windows user, but that is not the same thing as a process-level authorization boundary. Timeline reads therefore remain disabled unless the running Context Agent is explicitly given a read capability token.

## Server-side configuration

The agent reads the following environment variables once, on the first timeline read request, and keeps that policy in process memory for the rest of the process lifetime:

- `CONTEXT_LAYER_READ_TOKEN`: required to enable reads; 32–1024 bytes.
- `CONTEXT_LAYER_READ_PROFILE`: optional; `metadata` (default) or `sensitive`.
- `CONTEXT_LAYER_READ_SCOPES`: optional comma-separated exact scopes; defaults to `scope.personal`.

The client sends only the bearer token and the timeline query. It cannot request `Sensitive`, `Secret`, payload access, or additional scopes in the wire request. Those are chosen by the running server configuration.

`metadata` maps to the conservative internal grant: Metadata events only, Normal reference metadata only, no payload.

`sensitive` allows Sensitive events and Sensitive reference metadata and may return bounded JSON payloads. It still does **not** grant Secret content references.

There is intentionally no `secret` IPC profile in v1. Raw vault bytes also remain unavailable through the Local API.

## Wire limits

The Local API is framed at 1 MiB, so the read adapter applies a stricter response budget:

- maximum requested page: 20 visible entries;
- maximum returned content refs per entry: 32;
- maximum returned JSON payload per entry: 32 KiB;
- target serialized timeline page budget: 768 KiB.

Wire-level omission is explicit through `content_refs_omitted` and `payload_omitted_reason`. The internal retrieval engine remains lossless within its authorization policy; these additional limits exist only at the transport boundary.

## Token handling

The wire token is represented by `ReadCapabilityToken`. It serializes transparently for the protocol but its Rust `Debug` implementation always prints `[REDACTED]` so ordinary request debugging does not leak the credential.

The server compares tokens without data-dependent byte-by-byte early exit after the length check.

## Threat boundary

This capability prevents an ordinary Local API client that does not possess the token from simply asking for personal history. It is **not** a claim that bearer tokens create a strong sandbox against a determined malicious process already running under the same Windows user. Same-user process inspection, environment access, injection, and writable user files are a different OS isolation problem.

A stronger adversarial same-user boundary would require a protected broker / distinct Windows principal, AppContainer or packaged application identity, or another OS-backed isolation mechanism. v1 does not pretend otherwise.
