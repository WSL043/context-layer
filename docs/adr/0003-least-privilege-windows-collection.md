# ADR 0003: Standard-user Windows collection is the baseline

Status: Accepted.

## Context

USN Journal operations require administrator privileges and still do not identify the user task or foreground document. Making USN mandatory would turn an optional performance feature into an installation and trust boundary.

## Decision

The baseline watches only user-selected roots with `ReadDirectoryChangesExW`, resolves stable File IDs through file handles, and records explicit gap events before rescanning after overflow. A future USN adapter may be enabled separately after capability detection and explicit elevation.

## Consequences

- Normal installation and runtime do not require UAC.
- Watcher overflow and reconciliation are first-class contract cases.
- The event core remains independent of NTFS.
- USN failure cannot prevent the agent from starting.
