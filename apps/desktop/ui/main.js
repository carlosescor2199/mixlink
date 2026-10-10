// The MixLink server surface.
//
// There is no bundler and no Node toolchain: `window.__TAURI__` is present because tauri.conf.json
// sets `withGlobalTauri`, and everything below is plain DOM work. The engine is started once with
// no musicians configured, then polled a few times a second so the meters and counters stay alive.

const { invoke } = window.__TAURI__.core;
const currentWindow = window.__TAURI__.window.getCurrentWindow();

const POLL_INTERVAL_MS = 250;

const els = {
  engineState: document.getElementById("engine-state"),
  deviceSelect: document.getElementById("device-select"),
  deviceWarning: document.getElementById("device-warning"),
  deviceWarningText: document.getElementById("device-warning-text"),
  deviceSwitchAnyway: document.getElementById("device-switch-anyway"),
  deviceCancel: document.getElementById("device-cancel"),
  format: document.getElementById("format"),
  sampleRate: document.getElementById("sample-rate"),
  controlPort: document.getElementById("control-port"),
  samplesReceived: document.getElementById("samples-received"),
  packets: document.getElementById("packets"),
  channelCount: document.getElementById("channel-count"),
  groups: document.getElementById("groups"),
  channels: document.getElementById("channels"),
  addMusicianForm: document.getElementById("add-musician"),
  newTarget: document.getElementById("new-target"),
  musicianError: document.getElementById("musician-error"),
  musicianCount: document.getElementById("musician-count"),
  musicians: document.getElementById("musicians"),
  musiciansEmpty: document.getElementById("musicians-empty"),
};

// Channel number -> the DOM nodes whose values change every poll. The strips themselves are built
// once per channel count so the meter's CSS transition is not restarted on every poll.
const channelViews = new Map();

// Address -> the DOM nodes of one musician row. Rows are rebuilt only when the set of addresses
// changes, so an inline rename keeps its focus and the table never flickers on a poll.
const musicianViews = new Map();

// The device list from the engine, the last status snapshot, and the device the user is being asked
// to confirm a switch to. Kept in module scope so a poll never loses them.
let inputDevices = [];
let lastStatus = null;
let pendingDeviceName = null;

let pollTimer = null;

function describeFormat(format) {
  return `${format.channels} ch @ ${format.sampleRate} Hz, ${format.sampleFormat}, buffer ${format.bufferSize}`;
}

function formatNumber(value) {
  return Number(value).toLocaleString("en-US");
}

function setEngineState(text, variant) {
  els.engineState.textContent = text;
  els.engineState.className = `state state--${variant}`;
}

function renderChannels(channels) {
  if (channelViews.size !== channels.length) {
    channelViews.clear();
    els.channels.replaceChildren();
    for (const channel of channels) {
      const strip = document.createElement("div");
      strip.className = "strip";

      const head = document.createElement("div");
      head.className = "strip-head";
      const number = document.createElement("span");
      number.className = "strip-number mono";
      number.textContent = String(channel.number);
      const group = document.createElement("span");
      group.className = "strip-group";
      if (channel.group) {
        group.textContent = channel.group;
      } else {
        group.textContent = "—";
        group.classList.add("strip-group--none");
      }
      head.append(number, group);

      const meter = document.createElement("div");
      meter.className = "meter";
      const track = document.createElement("div");
      track.className = "meter-track";
      const fill = document.createElement("div");
      fill.className = "meter-fill";
      track.append(fill);
      meter.append(track);

      const level = document.createElement("div");
      level.className = "strip-level mono";

      strip.append(head, meter, level);
      els.channels.append(strip);
      channelViews.set(channel.number, { fill, level });
    }
  }

  for (const channel of channels) {
    const view = channelViews.get(channel.number);
    view.fill.style.height = `${channel.level}%`;
    view.level.textContent = String(channel.level);
  }
}

function renderGroups(groups) {
  if (groups.length === 0) {
    els.groups.hidden = true;
    els.groups.replaceChildren();
    return;
  }
  els.groups.hidden = false;
  const fragment = document.createDocumentFragment();
  for (const group of groups) {
    const chip = document.createElement("span");
    chip.className = group.invalid ? "chip chip--invalid" : "chip";
    const channels = group.channels.map((channel) => channel + 1).join(", ");
    chip.textContent = group.invalid
      ? `${group.name} — unavailable on this device`
      : group.name;
    chip.title = `Channels ${channels}`;
    fragment.append(chip);
  }
  els.groups.replaceChildren(fragment);
}

function renderDeviceOptions() {
  els.deviceSelect.replaceChildren();
  if (inputDevices.length === 0) {
    const option = document.createElement("option");
    option.value = "";
    option.textContent = "no input devices";
    els.deviceSelect.append(option);
    els.deviceSelect.disabled = true;
    return;
  }
  for (const device of inputDevices) {
    const option = document.createElement("option");
    option.value = device.name;
    option.textContent = `${device.name} — ${device.channels} ${
      device.channels === 1 ? "channel" : "channels"
    }`;
    option.disabled = device.channels === 0;
    els.deviceSelect.append(option);
  }
  syncDeviceSelection();
}

function syncDeviceSelection() {
  if (inputDevices.length === 0) {
    els.deviceSelect.disabled = true;
    return;
  }
  els.deviceSelect.disabled = lastStatus === null || lastStatus.stopped;
  if (lastStatus) {
    els.deviceSelect.value = lastStatus.deviceName;
  }
}

async function loadDevices() {
  try {
    inputDevices = await invoke("list_input_devices");
  } catch (error) {
    inputDevices = [];
    console.warn("could not list input devices", error);
  }
  renderDeviceOptions();
}

// The names of the groups the engine would mark invalid if the input had `channelCount` channels.
// Groups carry 0-based channels, so a channel is missing when it is at or beyond the new count.
function invalidGroupsFor(channelCount) {
  if (!lastStatus) {
    return [];
  }
  return lastStatus.groups
    .filter((group) => group.channels.some((channel) => channel >= channelCount))
    .map((group) => group.name);
}

function hideDeviceWarning() {
  pendingDeviceName = null;
  els.deviceWarning.hidden = true;
}

function requestDeviceSwitch(name) {
  if (!name || (lastStatus && name === lastStatus.deviceName)) {
    hideDeviceWarning();
    renderDeviceOptions();
    return;
  }
  const device = inputDevices.find((candidate) => candidate.name === name);
  if (!device) {
    return;
  }
  const invalid = invalidGroupsFor(device.channels);
  if (invalid.length > 0) {
    pendingDeviceName = name;
    const noun = invalid.length === 1 ? "group" : "groups";
    els.deviceWarningText.textContent =
      `Switching to ${device.name} (${device.channels} ch) makes ${invalid.length} ${noun} ` +
      `unavailable: ${invalid.join(", ")}. They are kept, not deleted, and become available again ` +
      `if you switch back.`;
    els.deviceWarning.hidden = false;
    renderDeviceOptions();
    return;
  }
  performDeviceSwitch(name);
}

async function performDeviceSwitch(name) {
  hideDeviceWarning();
  els.deviceSelect.disabled = true;
  try {
    const summary = await invoke("switch_device", { device: name });
    els.format.textContent = describeFormat(summary.captureFormat);
    try {
      await currentWindow.setTitle(`MixLink Server — ${summary.deviceName}`);
    } catch (error) {
      console.warn("could not update the window title", error);
    }
    await refreshStatus();
  } catch (error) {
    setEngineState(`switch failed: ${error}`, "error");
    await refreshStatus();
  }
}

function textCell(text, className) {
  const cell = document.createElement("td");
  cell.textContent = text;
  if (className) {
    cell.className = className;
  }
  return cell;
}

function showMusicianError(message) {
  els.musicianError.textContent = message;
  els.musicianError.hidden = false;
}

function hideMusicianError() {
  els.musicianError.textContent = "";
  els.musicianError.hidden = true;
}

// Builds one musician row. The name input commits on blur (Enter blurs it, Escape reverts first),
// and the poll never overwrites the input while it has focus.
function buildMusicianRow(musician) {
  const row = document.createElement("tr");

  const nameCell = document.createElement("td");
  const nameInput = document.createElement("input");
  nameInput.type = "text";
  nameInput.className = "name-input";
  nameInput.placeholder = "unnamed";
  nameInput.value = musician.name || "";
  nameInput.dataset.saved = musician.name || "";
  nameInput.title =
    "Local label for this desk; the phone's own name shows when this is empty. Enter saves, Escape reverts";
  nameInput.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      nameInput.blur();
    } else if (event.key === "Escape") {
      nameInput.value = nameInput.dataset.saved || "";
      nameInput.blur();
    }
  });
  nameInput.addEventListener("change", async () => {
    try {
      await invoke("set_musician_name", {
        address: musician.address,
        name: nameInput.value,
      });
      nameInput.dataset.saved = nameInput.value.trim();
      hideMusicianError();
    } catch (error) {
      showMusicianError(`could not save the name: ${error}`);
    }
  });
  nameCell.append(nameInput);

  const controlCell = document.createElement("td");
  const controlDot = document.createElement("span");
  const controlLabel = document.createElement("span");
  controlCell.append(controlDot, controlLabel);

  const mixCell = document.createElement("td");
  const mixVolume = document.createElement("span");
  mixVolume.className = "mono";
  const mixState = document.createElement("span");
  mixCell.append(mixVolume, mixState);

  const sent = textCell("0", "num");
  const discarded = textCell("0", "num");
  const loss = textCell("0.00%", "num");

  const actions = document.createElement("td");
  actions.className = "actions";
  const remove = document.createElement("button");
  remove.type = "button";
  remove.className = "button button--small button--danger";
  remove.textContent = "Remove";
  remove.addEventListener("click", () => removeMusician(musician.address));
  actions.append(remove);

  row.append(
    nameCell,
    textCell(musician.address, "mono"),
    controlCell,
    mixCell,
    sent,
    discarded,
    loss,
    actions,
  );
  return { row, nameInput, controlDot, controlLabel, mixVolume, mixState, sent, discarded, loss };
}

function updateMusicianRow(view, musician) {
  const total = musician.packetsSent + musician.packetsDiscarded;
  const loss = total === 0 ? 0 : (musician.packetsDiscarded / total) * 100;

  view.controlDot.className = musician.controlConnected ? "dot dot--on" : "dot";
  view.controlLabel.textContent = musician.controlConnected ? "connected" : "offline";
  view.controlLabel.className = musician.controlConnected ? "" : "tag";
  view.mixVolume.textContent = `${musician.volumePercent}%`;
  view.mixState.textContent = musician.muted ? " · muted" : " · live";
  view.mixState.className = musician.muted ? "tag--muted" : "tag--live";
  view.sent.textContent = formatNumber(musician.packetsSent);
  view.discarded.textContent = formatNumber(musician.packetsDiscarded);
  view.loss.textContent = `${loss.toFixed(2)}%`;
  view.loss.className = loss > 0 ? "num warn" : "num";

  if (document.activeElement !== view.nameInput) {
    const name = musician.name || "";
    view.nameInput.value = name;
    view.nameInput.dataset.saved = name;
  }
}

function renderMusicians(musicians) {
  els.musicianCount.textContent = `${musicians.length} ${
    musicians.length === 1 ? "musician" : "musicians"
  }`;

  if (musicians.length === 0) {
    musicianViews.clear();
    els.musicians.replaceChildren();
    els.musiciansEmpty.hidden = false;
    return;
  }
  els.musiciansEmpty.hidden = true;

  // Rebuild only when the set of addresses changed: a poll must not destroy an input mid-rename.
  const sameSet =
    musicianViews.size === musicians.length &&
    musicians.every((musician) => musicianViews.has(musician.address));
  if (!sameSet) {
    musicianViews.clear();
    const rows = document.createDocumentFragment();
    for (const musician of musicians) {
      const view = buildMusicianRow(musician);
      musicianViews.set(musician.address, view);
      rows.append(view.row);
    }
    els.musicians.replaceChildren(rows);
  }
  for (const musician of musicians) {
    updateMusicianRow(musicianViews.get(musician.address), musician);
  }
}

async function addMusician(event) {
  event.preventDefault();
  const address = els.newTarget.value.trim();
  if (address === "") {
    return;
  }
  try {
    await invoke("add_target", { address });
    els.newTarget.value = "";
    hideMusicianError();
    await refreshStatus();
  } catch (error) {
    showMusicianError(String(error));
  }
}

async function removeMusician(address) {
  try {
    await invoke("remove_target", { address });
    hideMusicianError();
    await refreshStatus();
  } catch (error) {
    showMusicianError(String(error));
  }
}

function applyStatus(status) {
  lastStatus = status;
  setEngineState(status.stopped ? "stopped" : "running", status.stopped ? "idle" : "on");
  els.format.textContent = describeFormat(status.captureFormat);
  els.sampleRate.textContent = `${formatNumber(status.sampleRate)} Hz`;
  els.controlPort.textContent = String(status.controlPort);
  els.samplesReceived.textContent = formatNumber(status.samplesReceived);
  els.packets.textContent = `${formatNumber(status.packetsSent)} / ${formatNumber(status.packetsDiscarded)}`;
  els.channelCount.textContent = `${status.sourceChannels} ${
    status.sourceChannels === 1 ? "channel" : "channels"
  }`;
  renderChannels(status.channels);
  renderGroups(status.groups);
  renderMusicians(status.musicians);
  syncDeviceSelection();
}

async function refreshStatus() {
  try {
    applyStatus(await invoke("engine_status"));
  } catch (error) {
    // The engine has not started or was stopped, so the snapshot call is the thing that fails. The
    // message is short and belongs on the state pill rather than a console nobody is watching.
    setEngineState(String(error), "error");
  }
}

function startPolling() {
  if (pollTimer !== null) {
    return;
  }
  refreshStatus();
  pollTimer = window.setInterval(refreshStatus, POLL_INTERVAL_MS);
}

// The window starts the engine with nobody configured: the musician list is editable at runtime,
// so the engineer adds real addresses instead of deleting a placeholder localhost target.
function defaultConfig() {
  return {
    deviceFilter: null,
    targets: [],
    controlPort: 50001,
    groups: [],
  };
}

async function startEngine() {
  setEngineState("starting", "idle");
  try {
    const summary = await invoke("start_engine", { config: defaultConfig() });
    els.format.textContent = describeFormat(summary.captureFormat);
    setEngineState("running", "on");

    // Mirror the selected device into the OS window title. Failure here must not hide the device
    // and format, so it is non-fatal.
    try {
      await currentWindow.setTitle(`MixLink Server — ${summary.deviceName}`);
    } catch (error) {
      console.warn("could not update the window title", error);
    }

    startPolling();
    await loadDevices();
  } catch (error) {
    setEngineState(`error: ${error}`, "error");
    els.format.textContent = "—";
  }
}

async function stopEngine() {
  try {
    await invoke("stop_engine");
    if (pollTimer !== null) {
      window.clearInterval(pollTimer);
      pollTimer = null;
    }
    lastStatus = null;
    hideDeviceWarning();
    hideMusicianError();
    els.deviceSelect.disabled = true;
    setEngineState("stopped", "idle");
  } catch (error) {
    setEngineState(`error: ${error}`, "error");
  }
}

document.getElementById("refresh").addEventListener("click", refreshStatus);
document.getElementById("stop").addEventListener("click", stopEngine);
els.deviceSelect.addEventListener("change", () => requestDeviceSwitch(els.deviceSelect.value));
els.deviceSwitchAnyway.addEventListener("click", () => {
  if (pendingDeviceName) {
    performDeviceSwitch(pendingDeviceName);
  }
});
els.deviceCancel.addEventListener("click", () => {
  hideDeviceWarning();
  renderDeviceOptions();
});
els.addMusicianForm.addEventListener("submit", addMusician);

loadDevices();
startEngine();
