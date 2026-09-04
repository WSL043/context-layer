export const HOST_NAME = "com.contextlayer.browser";
export const PROTOCOL_VERSION = 2;
export const LEGACY_PROTOCOL_VERSION = 1;

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
  validateWebUrl(url);
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

export function activePageChanged(
  page,
  observationId,
  sourceSequence,
  trigger,
  observedAt = new Date().toISOString(),
) {
  if (typeof observationId !== "string" || observationId.length < 32) {
    throw new Error("active-page observation UUID is required");
  }
  if (!Number.isSafeInteger(page?.tabId) || page.tabId < 0) {
    throw new Error("active-page tab id must be a non-negative safe integer");
  }
  if (!Number.isSafeInteger(page?.windowId) || page.windowId < 0) {
    throw new Error("active-page window id must be a non-negative safe integer");
  }
  validateWebUrl(page.url);
  if (typeof page.title !== "string" || page.title.length > 4096) {
    throw new Error("active-page title must be at most 4096 characters");
  }
  if (!["startup", "installed", "tab_activated", "page_updated", "window_focused", "window_blurred"].includes(trigger)) {
    throw new Error("unsupported active-page trigger");
  }
  return {
    type: "active_page_changed",
    protocol_version: PROTOCOL_VERSION,
    browser: "chromium",
    observation_id: observationId,
    source_sequence: sourceSequence,
    tab_id: page.tabId,
    window_id: page.windowId,
    url: page.url,
    title: page.title,
    pinned: Boolean(page.pinned),
    window_focused: Boolean(page.windowFocused),
    trigger,
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

export function responseAccepted(response, expectedProtocolVersion = PROTOCOL_VERSION) {
  return response?.protocol_version === expectedProtocolVersion
    && response?.result?.type === "event_accepted";
}

export function isWebUrl(value) {
  return typeof value === "string" && /^https?:\/\//i.test(value);
}

function validateWebUrl(value) {
  if (!isWebUrl(value) || value.length > 16_384) {
    throw new Error("URL must use HTTP or HTTPS and be at most 16384 characters");
  }
}
