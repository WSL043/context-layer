import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

import {
  ACTIVE_PAGE_PROTOCOL_VERSION,
  LEGACY_PROTOCOL_VERSION,
  PROTOCOL_VERSION,
  activePageChanged,
  collectorGap,
  downloadCompleted,
  isWebUrl,
  responseAccepted,
  textInteractionObserved,
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

test("checked-in browser v3 active-page fixture matches the JavaScript producer", () => {
  const fixture = JSON.parse(fs.readFileSync(
    new URL("../../../schemas/browser/v3/active_page_changed.json", import.meta.url),
    "utf8",
  ));
  const actual = activePageChanged(
    {
      tabId: 12,
      windowId: 3,
      url: "https://www.google.com/search?q=context+layer",
      title: "context layer - Google Search",
      pinned: false,
      windowFocused: true,
    },
    "018bcfe5-6800-7000-8000-000000000002",
    8,
    "tab_activated",
    "2026-09-01T00:00:01Z",
  );
  assert.deepEqual(actual, fixture);
});

test("browser v2 active-page fixture remains recognized as a legacy bridge shape", () => {
  const fixture = JSON.parse(fs.readFileSync(
    new URL("../../../schemas/browser/v2/active_page_changed.json", import.meta.url),
    "utf8",
  ));
  assert.equal(fixture.protocol_version, ACTIVE_PAGE_PROTOCOL_VERSION);
  assert.equal(fixture.type, "active_page_changed");
});

test("retained copy interaction matches checked-in v3 fixture", () => {
  const fixture = JSON.parse(fs.readFileSync(
    new URL("../../../schemas/browser/v3/text_interaction_observed.json", import.meta.url),
    "utf8",
  ));
  const actual = textInteractionObserved(
    {
      tabId: 12,
      windowId: 3,
      url: "https://example.test/research",
      title: "Research note",
      interaction: "copy",
      selectionStatus: "retained",
      selectedUtf8Bytes: 16,
      selectedText: "selected context",
      contextStatus: "retained",
      visibleContext: "A paragraph with selected context inside.",
      observedAt: "2026-09-01T00:00:02Z",
    },
    "018bcfe5-6800-7000-8000-000000000003",
    9,
  );
  assert.deepEqual(actual, fixture);
});

test("text interaction rejects inconsistent retained and omitted bodies", () => {
  const base = {
    tabId: 1,
    windowId: 2,
    url: "https://example.test/",
    title: "Example",
    interaction: "selection",
    selectionStatus: "retained",
    selectedUtf8Bytes: 3,
    selectedText: "four",
    contextStatus: "unavailable",
    visibleContext: null,
    observedAt: "2026-09-01T00:00:02Z",
  };
  assert.throws(() => textInteractionObserved(base, UUID, 1));
  assert.throws(() => textInteractionObserved({
    ...base,
    selectionStatus: "omitted_too_large",
    selectedUtf8Bytes: 65 * 1024,
    selectedText: "must-not-survive",
  }, UUID, 1));
  assert.throws(() => textInteractionObserved({
    ...base,
    selectedUtf8Bytes: 4,
    selectedText: "four",
    contextStatus: "retained",
    visibleContext: "x".repeat((16 * 1024) + 1),
  }, UUID, 1));
});

test("text interaction accepts metadata-only oversized selection", () => {
  const message = textInteractionObserved(
    {
      tabId: 1,
      windowId: 2,
      url: "https://example.test/",
      title: "Example",
      interaction: "selection",
      selectionStatus: "omitted_too_large",
      selectedUtf8Bytes: (64 * 1024) + 1,
      selectedText: null,
      contextStatus: "omitted_too_large",
      visibleContext: null,
      observedAt: "2026-09-01T00:00:02Z",
    },
    UUID,
    10,
  );
  assert.equal(message.selected_text, null);
  assert.equal(message.selection_status, "omitted_too_large");
});

test("active page rejects privileged and hostless URLs", () => {
  assert.equal(isWebUrl("https://example.test/"), true);
  assert.equal(isWebUrl("chrome://settings/"), false);
  assert.equal(isWebUrl("https://"), false);
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
    protocol_version: ACTIVE_PAGE_PROTOCOL_VERSION,
    result: { type: "event_accepted", duplicate: true },
  }, ACTIVE_PAGE_PROTOCOL_VERSION), true);
  assert.equal(responseAccepted({
    protocol_version: PROTOCOL_VERSION,
    result: { type: "error" },
  }), false);
});
