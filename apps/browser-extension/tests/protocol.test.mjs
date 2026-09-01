import assert from "node:assert/strict";
import fs from "node:fs";
import test from "node:test";

import {
  PROTOCOL_VERSION,
  collectorGap,
  downloadCompleted,
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

test("checked-in browser fixture matches JavaScript producer", () => {
  const fixture = JSON.parse(fs.readFileSync(
    new URL("../../../schemas/browser/v1/download_completed.json", import.meta.url),
    "utf8",
  ));
  const actual = downloadCompleted(
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
    "2026-09-01T00:00:00Z",
  );
  assert.deepEqual(actual, fixture);
});

test("outbox gap carries a stable UUID and last browser sequence", () => {
  const message = collectorGap(UUID, 99, "outbox full", "2026-09-01T00:00:00.000Z");
  assert.equal(message.type, "collector_gap");
  assert.equal(message.gap_id, UUID);
  assert.equal(message.last_source_sequence, 99);
});

test("only versioned event acknowledgements clear the outbox", () => {
  assert.equal(responseAccepted({
    protocol_version: PROTOCOL_VERSION,
    result: { type: "event_accepted", duplicate: true },
  }), true);
  assert.equal(responseAccepted({
    protocol_version: PROTOCOL_VERSION,
    result: { type: "error" },
  }), false);
});
