const MESSAGE_TYPE = "context_layer_text_interaction";
const MAX_SELECTED_TEXT_BYTES = 64 * 1024;
const MAX_CONTEXT_BYTES = 16 * 1024;
const encoder = new TextEncoder();
let selectionTimer = null;
let lastSelectionSignature = null;

function byteLength(value) {
  return encoder.encode(value).length;
}

function isEditableNode(node) {
  const element = node?.nodeType === Node.ELEMENT_NODE
    ? node
    : node?.parentElement;
  return Boolean(element?.closest("input, textarea, [contenteditable]:not([contenteditable='false'])"));
}

function currentSelection() {
  if (!document.hasFocus()) return null;
  const selection = window.getSelection();
  if (!selection || selection.rangeCount === 0 || selection.isCollapsed) return null;
  if (isEditableNode(selection.anchorNode) || isEditableNode(selection.focusNode)) return null;
  const text = selection.toString();
  if (!text) return null;
  return { selection, text };
}

function semanticContext(selection, selectedText) {
  const range = selection.getRangeAt(0);
  let node = range.commonAncestorContainer;
  if (node.nodeType !== Node.ELEMENT_NODE) node = node.parentElement;
  const element = node?.closest?.(
    "p, li, blockquote, pre, code, td, th, h1, h2, h3, h4, h5, h6",
  );
  if (!element || isEditableNode(element)) {
    return { status: "unavailable", text: null };
  }

  const text = typeof element.innerText === "string" ? element.innerText : "";
  if (!text || text === selectedText) {
    return { status: "unavailable", text: null };
  }
  if (byteLength(text) > MAX_CONTEXT_BYTES) {
    return { status: "omitted_too_large", text: null };
  }
  return { status: "retained", text };
}

function buildObservation(interaction) {
  const current = currentSelection();
  if (!current) return null;

  const selectedBytes = byteLength(current.text);
  const selectionStatus = selectedBytes <= MAX_SELECTED_TEXT_BYTES
    ? "retained"
    : "omitted_too_large";
  const context = semanticContext(current.selection, current.text);
  return {
    type: MESSAGE_TYPE,
    interaction,
    url: location.href,
    title: document.title || "",
    selected_text: selectionStatus === "retained" ? current.text : null,
    selected_utf8_bytes: selectedBytes,
    selection_status: selectionStatus,
    visible_context: context.text,
    context_status: context.status,
    observed_at: new Date().toISOString(),
  };
}

function sendObservation(interaction, deduplicate) {
  const observation = buildObservation(interaction);
  if (!observation) {
    if (deduplicate) lastSelectionSignature = null;
    return;
  }

  if (deduplicate) {
    const signature = JSON.stringify([
      observation.url,
      observation.selection_status,
      observation.selected_text,
      observation.visible_context,
    ]);
    if (signature === lastSelectionSignature) return;
    lastSelectionSignature = signature;
  }
  void chrome.runtime.sendMessage(observation).catch(() => undefined);
}

function scheduleSelectionObservation() {
  if (selectionTimer !== null) clearTimeout(selectionTimer);
  selectionTimer = setTimeout(() => {
    selectionTimer = null;
    sendObservation("selection", true);
  }, 200);
}

document.addEventListener("pointerup", (event) => {
  if (!event.isTrusted) return;
  scheduleSelectionObservation();
}, true);

document.addEventListener("keyup", (event) => {
  if (!event.isTrusted) return;
  scheduleSelectionObservation();
}, true);

document.addEventListener("copy", (event) => {
  if (!event.isTrusted) return;
  if (selectionTimer !== null) {
    clearTimeout(selectionTimer);
    selectionTimer = null;
  }
  sendObservation("copy", false);
}, true);
