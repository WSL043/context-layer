import {
  HOST_NAME,
  activePageChanged,
  collectorGap,
  downloadCompleted,
  isWebUrl,
  responseAccepted,
} from "./protocol.js";

const STATE_KEY = "deliveryStateV1";
const RETRY_ALARM = "context-layer-delivery-retry";
const MAX_OUTBOX_MESSAGES = 256;
let operationChain = Promise.resolve();

function emptyState() {
  return {
    outbox: {},
    downloadIds: {},
    gap: null,
    lastSequence: 0,
    lastActivePage: null,
  };
}

async function loadState() {
  const stored = await chrome.storage.local.get(STATE_KEY);
  return { ...emptyState(), ...(stored[STATE_KEY] || {}) };
}

async function saveState(state) {
  await chrome.storage.local.set({ [STATE_KEY]: state });
}

function sendNative(message) {
  return new Promise((resolve, reject) => {
    chrome.runtime.sendNativeMessage(HOST_NAME, message, (response) => {
      const error = chrome.runtime.lastError;
      if (error) {
        reject(new Error(error.message));
        return;
      }
      resolve(response);
    });
  });
}

function runExclusive(operation) {
  const next = operationChain.then(operation, operation);
  operationChain = next.catch(() => undefined);
  return next;
}

function noteGap(state, reason) {
  if (!state.gap) {
    state.gap = collectorGap(
      crypto.randomUUID(),
      state.lastSequence,
      reason,
    );
  } else {
    state.gap.last_source_sequence = state.lastSequence;
  }
}

function outboxKey(message) {
  if (message.type === "download_completed") return message.download_id;
  if (message.type === "active_page_changed") return message.observation_id;
  throw new Error(`unsupported outbox message type: ${message.type}`);
}

function pageFromTab(tab, windowFocused) {
  if (!tab?.active || !Number.isSafeInteger(tab.id) || !Number.isSafeInteger(tab.windowId)) {
    return null;
  }
  if (!isWebUrl(tab.url)) return null;
  return {
    tabId: tab.id,
    windowId: tab.windowId,
    url: tab.url,
    title: typeof tab.title === "string" ? tab.title : "",
    pinned: Boolean(tab.pinned),
    windowFocused: Boolean(windowFocused),
  };
}

function pageSignature(page) {
  return JSON.stringify([
    page.tabId,
    page.windowId,
    page.url,
    page.title,
    page.pinned,
    page.windowFocused,
  ]);
}

async function stableDownloadId(browserDownloadId) {
  return runExclusive(async () => {
    const state = await loadState();
    const key = String(browserDownloadId);
    if (!state.downloadIds[key]) {
      state.downloadIds[key] = crypto.randomUUID();
      await saveState(state);
    }
    return state.downloadIds[key];
  });
}

async function enqueueCompleted(item) {
  return runExclusive(async () => {
    const state = await loadState();
    const key = String(item.id);
    const downloadId = state.downloadIds[key] || crypto.randomUUID();
    state.downloadIds[key] = downloadId;
    if (!state.outbox[downloadId]) {
      state.lastSequence += 1;
      if (Object.keys(state.outbox).length >= MAX_OUTBOX_MESSAGES) {
        noteGap(state, "browser delivery outbox reached its 256-message safety limit");
        delete state.downloadIds[key];
      } else {
        state.outbox[downloadId] = downloadCompleted(
          item,
          downloadId,
          state.lastSequence,
        );
      }
    }
    await saveState(state);
    await flushState();
  });
}

async function enqueueActivePage(page, trigger) {
  if (!page) return;
  return runExclusive(async () => {
    const state = await loadState();
    const signature = pageSignature(page);
    if (state.lastActivePage?.signature === signature) return;

    state.lastActivePage = { ...page, signature };
    state.lastSequence += 1;
    const observationId = crypto.randomUUID();
    if (Object.keys(state.outbox).length >= MAX_OUTBOX_MESSAGES) {
      noteGap(state, "browser delivery outbox reached its 256-message safety limit");
    } else {
      state.outbox[observationId] = activePageChanged(
        page,
        observationId,
        state.lastSequence,
        trigger,
      );
    }
    await saveState(state);
    await flushState();
  });
}

async function enqueueWindowBlurred() {
  return runExclusive(async () => {
    const state = await loadState();
    const previous = state.lastActivePage;
    if (!previous?.windowFocused) return;

    const page = {
      tabId: previous.tabId,
      windowId: previous.windowId,
      url: previous.url,
      title: previous.title,
      pinned: previous.pinned,
      windowFocused: false,
    };
    const signature = pageSignature(page);
    state.lastActivePage = { ...page, signature };
    state.lastSequence += 1;
    const observationId = crypto.randomUUID();
    if (Object.keys(state.outbox).length >= MAX_OUTBOX_MESSAGES) {
      noteGap(state, "browser delivery outbox reached its 256-message safety limit");
    } else {
      state.outbox[observationId] = activePageChanged(
        page,
        observationId,
        state.lastSequence,
        "window_blurred",
      );
    }
    await saveState(state);
    await flushState();
  });
}

async function flushOutbox() {
  return runExclusive(flushState);
}

async function flushState() {
  try {
    const state = await loadState();
    if (state.gap) {
      const response = await sendNative(state.gap);
      if (!responseAccepted(response, state.gap.protocol_version)) {
        throw new Error("agent rejected collector gap");
      }
      state.gap = null;
      await saveState(state);
    }

    const queued = Object.values(state.outbox)
      .sort((left, right) => left.source_sequence - right.source_sequence);
    for (const message of queued) {
      const response = await sendNative(message);
      if (!responseAccepted(response, message.protocol_version)) {
        throw new Error(`agent rejected ${message.type}`);
      }
      delete state.outbox[outboxKey(message)];
      if (message.type === "download_completed") {
        delete state.downloadIds[String(message.browser_download_id)];
      }
      await saveState(state);
    }
    await chrome.alarms.clear(RETRY_ALARM);
  } catch {
    await chrome.alarms.create(RETRY_ALARM, { delayInMinutes: 1 });
  }
}

async function observeCurrentFocusedPage(trigger) {
  try {
    const window = await chrome.windows.getLastFocused();
    if (!window?.focused || !Number.isSafeInteger(window.id)) {
      await enqueueWindowBlurred();
      return;
    }
    const tabs = await chrome.tabs.query({ active: true, windowId: window.id });
    await enqueueActivePage(pageFromTab(tabs[0], true), trigger);
  } catch {
    // Browser shutdown/race conditions are expected here; the next state event retries observation.
  }
}

chrome.downloads.onCreated.addListener((item) => {
  void stableDownloadId(item.id);
});

chrome.downloads.onChanged.addListener((delta) => {
  if (delta.state?.current !== "complete") return;
  void chrome.downloads.search({ id: delta.id }).then((items) => {
    const item = items[0];
    if (item?.state === "complete") return enqueueCompleted(item);
    return undefined;
  });
});

chrome.tabs.onActivated.addListener((activeInfo) => {
  void Promise.all([
    chrome.tabs.get(activeInfo.tabId),
    chrome.windows.get(activeInfo.windowId),
  ]).then(([tab, window]) => {
    if (!window.focused) return undefined;
    return enqueueActivePage(pageFromTab(tab, true), "tab_activated");
  }).catch(() => undefined);
});

chrome.tabs.onUpdated.addListener((_tabId, changeInfo, tab) => {
  if (!tab.active) return;
  if (changeInfo.url === undefined && changeInfo.title === undefined && changeInfo.status !== "complete") {
    return;
  }
  void chrome.windows.get(tab.windowId).then((window) => {
    if (!window.focused) return undefined;
    return enqueueActivePage(pageFromTab(tab, true), "page_updated");
  }).catch(() => undefined);
});

chrome.windows.onFocusChanged.addListener((windowId) => {
  if (windowId === chrome.windows.WINDOW_ID_NONE) {
    void enqueueWindowBlurred();
    return;
  }
  void chrome.tabs.query({ active: true, windowId }).then((tabs) => (
    enqueueActivePage(pageFromTab(tabs[0], true), "window_focused")
  )).catch(() => undefined);
});

chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === RETRY_ALARM) void flushOutbox();
});

chrome.runtime.onStartup.addListener(() => {
  void flushOutbox();
  void observeCurrentFocusedPage("startup");
});
chrome.runtime.onInstalled.addListener(() => {
  void flushOutbox();
  void observeCurrentFocusedPage("installed");
});
void flushOutbox();
