// The window's whole job in T3: ask the engine for its device and capture format, and show them.
// `window.__TAURI__` is available because tauri.conf.json sets `withGlobalTauri`, which keeps the
// app free of any Node toolchain.

const { invoke } = window.__TAURI__.core;
const currentWindow = window.__TAURI__.window.getCurrentWindow();

const deviceElement = document.getElementById("device");
const formatElement = document.getElementById("format");
const statusElement = document.getElementById("status");

function describeFormat(format) {
  return `${format.channels} ch @ ${format.sampleRate} Hz, ${format.sampleFormat}, buffer ${format.bufferSize}`;
}

// T3 only needs one configuration: the engine's own defaults. Editing is increment 2.
function defaultConfig() {
  return {
    deviceFilter: null,
    targets: ["127.0.0.1:50000"],
    controlPort: 50001,
    groups: [],
  };
}

async function startEngine() {
  statusElement.textContent = "Starting engine…";
  try {
    const summary = await invoke("start_engine", { config: defaultConfig() });
    deviceElement.textContent = summary.deviceName;
    formatElement.textContent = describeFormat(summary.captureFormat);
    statusElement.textContent = "Engine running.";

    // Mirror the travelled value into the OS window title so it is visible even while the body is
    // being reviewed. Failure here must not hide the device and format, so it is non-fatal.
    try {
      await currentWindow.setTitle(`MixLink Server — ${summary.deviceName}`);
    } catch (error) {
      console.warn("could not update the window title", error);
    }
  } catch (error) {
    statusElement.textContent = `Engine error: ${error}`;
    deviceElement.textContent = "—";
    formatElement.textContent = "—";
  }
}

async function refreshStatus() {
  try {
    const status = await invoke("engine_status");
    deviceElement.textContent = status.deviceName;
    formatElement.textContent = describeFormat(status.captureFormat);
    statusElement.textContent = status.stopped ? "Engine stopped." : "Engine running.";
  } catch (error) {
    statusElement.textContent = `Engine error: ${error}`;
  }
}

async function stopEngine() {
  try {
    await invoke("stop_engine");
    statusElement.textContent = "Engine stopped.";
  } catch (error) {
    statusElement.textContent = `Could not stop the engine: ${error}`;
  }
}

document.getElementById("refresh").addEventListener("click", refreshStatus);
document.getElementById("stop").addEventListener("click", stopEngine);

startEngine();
