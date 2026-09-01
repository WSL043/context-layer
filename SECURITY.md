# Security policy

## Supported versions

This project is pre-release. Only the latest commit on `main` will receive security fixes until the first tagged alpha.

## Reporting a vulnerability

Do not open a public issue for a vulnerability that could expose local activity, file metadata, update-signing material, or Native Messaging trust boundaries. Use GitHub private vulnerability reporting after the public repository is created. Until then, report privately to the repository owner.

Reports should include affected revision, platform, reproduction steps, expected boundary, observed impact, and whether the issue requires the same-user session, administrator access, or a malicious document.

## Security boundary

The foundation aims for current-user isolation, explicit collection scopes, signed updates, and no network service. It does not claim to resist malware already executing as the same Windows user.
