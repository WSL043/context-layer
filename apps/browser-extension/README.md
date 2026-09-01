# Chromium extension alpha

This unpacked Manifest V3 extension observes completed downloads and sends only
the browser download ID, stable event UUID, final HTTP(S) URL, referrer, absolute
local filename, and completion time to `com.contextlayer.browser`.

It has no content scripts or host permissions. A bounded durable outbox retries
when the local host or agent is unavailable. Retries keep the same event UUID;
capacity loss is reported as an explicit collector-gap event.

The Chrome downloads API documents `filename` as an absolute local path and the
download ID as persistent across browser sessions:
https://developer.chrome.com/docs/extensions/reference/api/downloads

Chrome/Edge Native Messaging requires the `nativeMessaging` permission and an
installed host manifest whose `allowed_origins` contains the actual extension ID:
https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging
https://learn.microsoft.com/en-us/microsoft-edge/extensions/developer-guide/native-messaging

This directory can be loaded unpacked for contract development. A real end-to-end
browser run still requires substituting its generated extension ID into the host
manifest and allowlist, registering that manifest under HKCU, and running the
agent. Those writes belong to the installer phase and are not performed by tests.
