import {
  HOST_NAME,
  collectorGap,
  downloadCompleted,
  responseAccepted,
} from "./protocol.js";

const STATE_KEY = "deliveryStateV1";
const RETRY_ALARM = "context-layer-delivery-retry";
const MAX_OUTBOX_MESSAGES = 256;
let operationChain = Promise.resolve();

function emptyState() {
  return { outbox: {}, downloadIds: {}, gap: null, lastSequence: 0 };
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
        if (!state.gap) {
          state.gap = collectorGap(
            crypto.randomUUID(),
            state.lastSequence,
            "browser delivery outbox reached its 256-message safety limit",
          );
        } else {
          state.gap.last_source_sequence = state.lastSequence;
        }
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

async function flushOutbox() {
  return runExclusive(flushState);
}

async function flushState() {
  try {
    const state = await loadState();
    if (state.gap) {
      const response = await sendNative(state.gap);
      if (!responseAccepted(response)) throw new Error("agent rejected collector gap");
      state.gap = null;
      await saveState(state);
    }

    const queued = Object.values(state.outbox)
      .sort((left, right) => left.source_sequence - right.source_sequence);
    for (const message of queued) {
      const response = await sendNative(message);
      if (!responseAccepted(response)) throw new Error("agent rejected download event");
      delete state.outbox[message.download_id];
      delete state.downloadIds[String(message.browser_download_id)];
      await saveState(state);
    }
    await chrome.alarms.clear(RETRY_ALARM);
  } catch {
    await chrome.alarms.create(RETRY_ALARM, { delayInMinutes: 1 });
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

chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === RETRY_ALARM) void flushOutbox();
});

chrome.runtime.onStartup.addListener(() => void flushOutbox());
chrome.runtime.onInstalled.addListener(() => void flushOutbox());
void flushOutbox();
