# Contract fixtures

Each event schema version keeps reviewed serialized fixtures. Fixtures are compatibility inputs, not examples generated from the current structs at test time.

Rules:

- Existing versioned fixture files are immutable.
- Additive compatible fields require a new fixture in the same version.
- A breaking field or semantic change requires a new schema-version directory and a migration/compatibility ADR.
- Fixtures contain synthetic paths and URLs only.
