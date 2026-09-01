export const HOST_NAME = "com.contextlayer.browser";
export const PROTOCOL_VERSION = 1;

export function downloadCompleted(
  item,
  downloadId,
  sourceSequence,
  observedAt = new Date().toISOString(),
) {
  if (!item || item.state !== "complete") {
    throw new Error("download item must be complete");
  }
  if (!Number.isSafeInteger(item.id) || item.id < 0) {
    throw new Error("browser download id must be a non-negative safe integer");
  }
  if (typeof downloadId !== "string" || downloadId.length < 32) {
    throw new Error("download UUID is required");
  }
  if (typeof item.filename !== "string" || item.filename.length === 0) {
    throw new Error("download filename is required");
  }
  const url = item.finalUrl || item.url;
  if (typeof url !== "string" || !/^https?:\/\//i.test(url)) {
    throw new Error("download URL must use HTTP or HTTPS");
  }
  return {
    type: "download_completed",
    protocol_version: PROTOCOL_VERSION,
    browser: "chromium",
    browser_download_id: item.id,
    source_sequence: sourceSequence,
    download_id: downloadId,
    url,
    referrer: item.referrer || null,
    final_path: item.filename,
    observed_at: observedAt,
  };
}

export function collectorGap(
  gapId,
  lastSourceSequence,
  reason,
  observedAt = new Date().toISOString(),
) {
  if (typeof gapId !== "string" || gapId.length < 32) {
    throw new Error("gap UUID is required");
  }
  return {
    type: "collector_gap",
    protocol_version: PROTOCOL_VERSION,
    browser: "chromium",
    gap_id: gapId,
    last_source_sequence: lastSourceSequence ?? null,
    reason,
    observed_at: observedAt,
  };
}

export function responseAccepted(response) {
  return response?.protocol_version === PROTOCOL_VERSION
    && response?.result?.type === "event_accepted";
}
