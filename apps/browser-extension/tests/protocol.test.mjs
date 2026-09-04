import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

import {
  LEGACY_PROTOCOL_VERSION,
  PROTOCOL_VERSION,
  activePageChanged,
  collectorGap,
  downloadCompleted,
  isWebUrl,
  responseAccepted,
} from "../protocol.js";

const UUID = "018bcfe5-6800-7000-8000-000000000001";

test("completed download maps absolute provenance fields", () => {
  const message = downloadCompleted(
    {
      id: 42,
      state: "complete",
      url: "https://example.test/original",
      finalUrl: "https://cdn.example.test/report.pdf",
      referrer: "https://example.test/",
      filename: "C:\\Users\\Example\\Downloads\\report.pdf",
    },
    UUID,
    7,
    "2026-09-01T00:00:00.000Z",
  );

  assert.equal(message.type, "download_completed");
  assert.equal(message.protocol_version, PROTOCOL_VERSION);
  assert.equal(message.browser_download_id, 42);
  assert.equal(message.source_sequence, 7);
  assert.equal(message.download_id, UUID);
  assert.equal(message.url, "https://cdn.example.test/report.pdf");
});

test("active page carries focused web state without page contents", () => {
  const message = activePageChanged(
    {
      tabId: 12,
      windowId: 3,
      url: "https://www.google.com/search?q=context+layer",
      title: "context layer - Google Search",
      pinned: false,
      windowFocused: true,
    },
    UUID,
    8,
    "tab_activated",
    "2026-09-01T00:00:01.000Z",
  );

  assert.equal(message.type, "active_page_changed");
  assert.equal(message.protocol_version, PROTOCOL_VERSION);
  assert.equal(message.observation_id, UUID);
  assert.equal(message.source_sequence, 8);
  assert.equal(message.url, "https://www.google.com/search?q=context+layer");
  assert.equal(message.window_focused, true);
  assert.equal(message.trigger, "tab_activated");
  assert.equal("content" in message, false);
});

test("active page rejects privileged browser URLs", () => {
  assert.equal(isWebUrl("https://example.test/"), true);
  assert.equal(isWebUrl("chrome://settings/"), false);
  assert.throws(() => activePageChanged(
    {
      tabId: 1,
      windowId: 1,
      url: "chrome://settings/",
      title: "Settings",
      pinned: false,
      windowFocused: true,
    },
    UUID,
    1,
    "page_updated",
  ));
});

test("checked-in browser v1 fixture remains readable by the JavaScript shape", () => {
  const fixture = JSON.parse(fs.readFileSync(
    new URL("../../../schemas/browser/v1/download_completed.json", import.meta.url),
    "utf8",
  ));
  assert.equal(fixture.protocol_version, LEGACY_PROTOCOL_VERSION);
  assert.equal(fixture.type, "download_completed");
});

test("outbox gap carries a stable UUID and last browser sequence", () => {
  const message = collectorGap(UUID, 99, "outbox full", "2026-09-01T00:00:00.000Z");
  assert.equal(message.type, "collector_gap");
  assert.equal(message.protocol_version, PROTOCOL_VERSION);
  assert.equal(message.gap_id, UUID);
  assert.equal(message.last_source_sequence, 99);
});

test("acknowledgements are checked against the message bridge version", () => {
  assert.equal(responseAccepted({
    protocol_version: PROTOCOL_VERSION,
    result: { type: "event_accepted", duplicate: true },
  }), true);
  assert.equal(responseAccepted({
    protocol_version: LEGACY_PROTOCOL_VERSION,
    result: { type: "event_accepted", duplicate: true },
  }, LEGACY_PROTOCOL_VERSION), true);
  assert.equal(responseAccepted({
    protocol_version: PROTOCOL_VERSION,
    result: { type: "error" },
  }), false);
});
