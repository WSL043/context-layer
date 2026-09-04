# Browser bridge v2

Browser bridge protocol v2 adds durable active-page state without changing the Context Agent Local API version.

The extension and Native Host use one monotonic `source_sequence` across current browser observations. In bridge v2, downloads, active-page observations, and collector gaps therefore belong to the same `scope.personal` collector stream. Legacy bridge-v1 download messages remain accepted and retain their historical `scope.downloads` behavior so an existing durable outbox can drain safely after upgrade.

`active_page_changed.json` is the compatibility fixture for active HTTP(S) page state. It records URL/title metadata, tab/window identity, browser-focus state, and the trigger that caused the observation. It does not represent DOM content and it must not be interpreted as proof that a background tab was viewed.
