# Windows packaging boundary

The eventual per-user installer substitutes an absolute escaped host path and the
published Chromium extension ID into the two files under `native-messaging/`.
It installs the resulting manifest and runtime allowlist beside
`context-native-host.exe`.

Registration is per-user under the browser vendor's Native Messaging Host HKCU
key. The installer must not request administrator rights for this baseline. It
must remove only the keys and files it created, and must ask separately before
removing the user's local Context Layer database.

These are templates, not a ready installer. Signing identity, extension IDs,
Tauri shell, uninstall behavior, and artifact verification remain release gates.
