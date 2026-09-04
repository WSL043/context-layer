export const HOST_NAME = "com.contextlayer.browser";
export const PROTOCOL_VERSION = 3;
export const LEGACY_PROTOCOL_VERSION = 1;
export const ACTIVE_PAGE_PROTOCOL_VERSION = 2;

const encoder = new TextEncoder();
const MAX_SELECTED_TEXT_BYTES = 64 * 1024;
const MAX_CONTEXT_BYTES = 16 * 1024;

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
  if (item.referrer) validateWebUrl(item.referrer);
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
  validateTitle(page.title, "active-page");
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

export function textInteractionObserved(
  observation,
  observationId,
  sourceSequence,
) {
  if (typeof observationId !== "string" || observationId.length < 32) {
    throw new Error("text-interaction observation UUID is required");
  }
  if (!Number.isSafeInteger(observation?.tabId) || observation.tabId < 0) {
    throw new Error("text-interaction tab id must be a non-negative safe integer");
  }
  if (!Number.isSafeInteger(observation?.windowId) || observation.windowId < 0) {
    throw new Error("text-interaction window id must be a non-negative safe integer");
  }
  if (!["selection", "copy"].includes(observation.interaction)) {
    throw new Error("unsupported text interaction");
  }
  validateWebUrl(observation.url);
  validateTitle(observation.title, "text-interaction");

  const selectedBytes = observation.selectedUtf8Bytes;
  if (!Number.isSafeInteger(selectedBytes) || selectedBytes <= 0) {
    throw new Error("selected UTF-8 byte length must be a positive safe integer");
  }
  if (observation.selectionStatus === "retained") {
    if (typeof observation.selectedText !== "string" || observation.selectedText.length === 0) {
      throw new Error("retained selection requires selected text");
    }
    const actualBytes = encoder.encode(observation.selectedText).length;
    if (actualBytes !== selectedBytes || actualBytes > MAX_SELECTED_TEXT_BYTES) {
      throw new Error("retained selected text byte length is invalid");
    }
  } else if (observation.selectionStatus === "omitted_too_large") {
    if (observation.selectedText !== null || selectedBytes <= MAX_SELECTED_TEXT_BYTES) {
      throw new Error("oversized selection must omit text and report its original byte length");
    }
  } else {
    throw new Error("unsupported selection status");
  }

  if (observation.contextStatus === "retained") {
    if (typeof observation.visibleContext !== "string" || observation.visibleContext.length === 0) {
      throw new Error("retained context requires visible context text");
    }
    if (encoder.encode(observation.visibleContext).length > MAX_CONTEXT_BYTES) {
      throw new Error("visible context exceeds the 16384-byte limit");
    }
  } else if (["unavailable", "omitted_too_large"].includes(observation.contextStatus)) {
    if (observation.visibleContext !== null) {
      throw new Error("non-retained context must omit context text");
    }
  } else {
    throw new Error("unsupported context status");
  }

  return {
    type: "text_interaction_observed",
    protocol_version: PROTOCOL_VERSION,
    browser: "chromium",
    observation_id: observationId,
    source_sequence: sourceSequence,
    tab_id: observation.tabId,
    window_id: observation.windowId,
    url: observation.url,
    title: observation.title,
    interaction: observation.interaction,
    selection_status: observation.selectionStatus,
    selected_utf8_bytes: selectedBytes,
    selected_text: observation.selectedText,
    context_status: observation.contextStatus,
    visible_context: observation.visibleContext,
    observed_at: observation.observedAt,
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
  try {
    validateWebUrl(value);
    return true;
  } catch {
    return false;
  }
}

function validateTitle(value, prefix) {
  if (typeof value !== "string" || encoder.encode(value).length > 4096) {
    throw new Error(`${prefix} title must be at most 4096 UTF-8 bytes`);
  }
}

function validateWebUrl(value) {
  if (typeof value !== "string" || encoder.encode(value).length > 16_384) {
    throw new Error("URL must be at most 16384 UTF-8 bytes");
  }
  let parsed;
  try {
    parsed = new URL(value);
  } catch {
    throw new Error("URL must be valid HTTP or HTTPS");
  }
  if (!/^https?:$/i.test(parsed.protocol) || !parsed.hostname) {
    throw new Error("URL must use HTTP or HTTPS and include a host");
  }
}
