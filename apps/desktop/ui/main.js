// The MixLink server surface.
//
// There is no bundler and no Node toolchain: `window.__TAURI__` is present because tauri.conf.json
// sets `withGlobalTauri`, and everything below is plain DOM work. The engine is started once with
// its own defaults, then polled a few times a second so the meters and counters stay alive.

const { invoke } = window.__TAURI__.core;
const currentWindow = window.__TAURI__.window.getCurrentWindow();

const POLL_INTERVAL_MS = 250;

const els = {
  engineState: document.getElementById("engine-state"),
  device: document.getElementById("device"),
  format: document.getElementById("format"),
  sampleRate: document.getElementById("sample-rate"),
  controlPort: document.getElementById("control-port"),
  samplesReceived: document.getElementById("samples-received"),
  packets: document.getElementById("packets"),
  channelCount: document.getElementById("channel-count"),
  channels: document.getElementById("channels"),
  musicianCount: document.getElementById("musician-count"),
  musicians: document.getElementById("musicians"),
  musiciansEmpty: document.getElementById("musicians-empty"),
};

// Channel number -> the DOM nodes whose values change every poll. The strips themselves are built
// once per channel count so the meter's CSS transition is not restarted on every poll.
const channelViews = new Map();

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

function textCell(text, className) {
  const cell = document.createElement("td");
  cell.textContent = text;
  if (className) {
    cell.className = className;
  }
  return cell;
}

function mixCell(musician) {
  const cell = document.createElement("td");
  const volume = document.createElement("span");
  volume.className = "mono";
  volume.textContent = `${musician.volumePercent}%`;
  const state = document.createElement("span");
  state.className = musician.muted ? "tag--muted" : "tag--live";
  state.textContent = musician.muted ? " · muted" : " · live";
  cell.append(volume, state);
  return cell;
}

function controlCell(connected) {
  const cell = document.createElement("td");
  const dot = document.createElement("span");
  dot.className = connected ? "dot dot--on" : "dot";
  const label = document.createElement("span");
  label.textContent = connected ? "connected" : "offline";
  if (!connected) {
    label.className = "tag";
  }
  cell.append(dot, label);
  return cell;
}

function renderMusicians(musicians) {
  els.musicianCount.textContent = `${musicians.length} configured`;

  if (musicians.length === 0) {
    els.musicians.replaceChildren();
    els.musiciansEmpty.hidden = false;
    return;
  }
  els.musiciansEmpty.hidden = true;

  const rows = document.createDocumentFragment();
  for (const musician of musicians) {
    const total = musician.packetsSent + musician.packetsDiscarded;
    const loss = total === 0 ? 0 : (musician.packetsDiscarded / total) * 100;

    const row = document.createElement("tr");
    row.append(
      textCell(musician.address, "mono"),
      controlCell(musician.controlConnected),
      mixCell(musician),
      textCell(formatNumber(musician.packetsSent), "num"),
      textCell(formatNumber(musician.packetsDiscarded), "num"),
      textCell(`${loss.toFixed(2)}%`, loss > 0 ? "num warn" : "num"),
    );
    rows.append(row);
  }
  els.musicians.replaceChildren(rows);
}

function applyStatus(status) {
  setEngineState(status.stopped ? "stopped" : "running", status.stopped ? "idle" : "on");
  els.device.textContent = status.deviceName;
  els.format.textContent = describeFormat(status.captureFormat);
  els.sampleRate.textContent = `${formatNumber(status.sampleRate)} Hz`;
  els.controlPort.textContent = String(status.controlPort);
  els.samplesReceived.textContent = formatNumber(status.samplesReceived);
  els.packets.textContent = `${formatNumber(status.packetsSent)} / ${formatNumber(status.packetsDiscarded)}`;
  els.channelCount.textContent = `${status.sourceChannels} ${
    status.sourceChannels === 1 ? "channel" : "channels"
  }`;
  renderChannels(status.channels);
  renderMusicians(status.musicians);
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

// The window only drives the engine's own defaults in this increment; editing is a later one.
function defaultConfig() {
  return {
    deviceFilter: null,
    targets: ["127.0.0.1:50000"],
    controlPort: 50001,
    groups: [],
  };
}

async function startEngine() {
  setEngineState("starting", "idle");
  try {
    const summary = await invoke("start_engine", { config: defaultConfig() });
    els.device.textContent = summary.deviceName;
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
  } catch (error) {
    setEngineState(`error: ${error}`, "error");
    els.device.textContent = "—";
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
    setEngineState("stopped", "idle");
  } catch (error) {
    setEngineState(`error: ${error}`, "error");
  }
}

document.getElementById("refresh").addEventListener("click", refreshStatus);
document.getElementById("stop").addEventListener("click", stopEngine);

startEngine();
