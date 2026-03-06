import { invoke } from "@tauri-apps/api/core";

type RenderSnapshot = {
  current_url: string;
  title: string;
  text_blocks: string[];
  diagnostics: string[];
};

let urlInputEl: HTMLInputElement | null;
let statusEl: HTMLElement | null;
let currentUrlEl: HTMLElement | null;
let textListEl: HTMLUListElement | null;

function setStatus(message: string) {
  if (statusEl) {
    statusEl.textContent = message;
  }
}

function renderSnapshot(snapshot: RenderSnapshot) {
  if (currentUrlEl) {
    currentUrlEl.textContent = snapshot.current_url
      ? `URL: ${snapshot.current_url}`
      : "URL: (none)";
  }

  if (!textListEl) {
    return;
  }

  textListEl.innerHTML = "";

  const lines =
    snapshot.text_blocks.length > 0
      ? snapshot.text_blocks
      : snapshot.diagnostics.length > 0
      ? snapshot.diagnostics
      : ["No content"];

  for (const line of lines) {
    const item = document.createElement("li");
    item.textContent = line;
    textListEl.appendChild(item);
  }
}

async function openCurrentUrl() {
  if (!urlInputEl) {
    return;
  }

  const url = urlInputEl.value.trim();
  if (!url) {
    setStatus("URL を入力してください。");
    return;
  }

  try {
    setStatus("Loading...");
    const snapshot = await invoke<RenderSnapshot>("open_url", { url });
    renderSnapshot(snapshot);
    setStatus("Done.");
  } catch (e) {
    setStatus(`Error: ${e}`);
  }
}

async function loadInitialSnapshot() {
  try {
    const snapshot = await invoke<RenderSnapshot>("get_render_snapshot");
    renderSnapshot(snapshot);
  } catch {
    setStatus("初期スナップショットの取得に失敗しました。");
  }
}

window.addEventListener("DOMContentLoaded", () => {
  urlInputEl = document.querySelector("#url-input");
  statusEl = document.querySelector("#status");
  currentUrlEl = document.querySelector("#current-url");
  textListEl = document.querySelector("#text-list");

  document.querySelector("#url-form")?.addEventListener("submit", (event) => {
    event.preventDefault();
    openCurrentUrl();
  });

  loadInitialSnapshot();
});
