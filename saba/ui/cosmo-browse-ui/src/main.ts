import { invoke } from "@tauri-apps/api/core";

type RenderSnapshot = {
  current_url: string;
  title: string;
  text_blocks: string[];
  diagnostics: string[];
};

type NavigationState = {
  can_back: boolean;
  can_forward: boolean;
  current_url?: string;
};

let urlInputEl: HTMLInputElement | null;
let statusEl: HTMLElement | null;
let currentUrlEl: HTMLElement | null;
let textListEl: HTMLUListElement | null;
let backBtnEl: HTMLButtonElement | null;
let forwardBtnEl: HTMLButtonElement | null;

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

  if (urlInputEl && snapshot.current_url) {
    urlInputEl.value = snapshot.current_url;
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

function applyNavigationState(nav: NavigationState) {
  if (backBtnEl) {
    backBtnEl.disabled = !nav.can_back;
  }
  if (forwardBtnEl) {
    forwardBtnEl.disabled = !nav.can_forward;
  }
}

async function refreshNavigationState() {
  try {
    const nav = await invoke<NavigationState>("get_navigation_state");
    applyNavigationState(nav);
  } catch {
    if (backBtnEl) backBtnEl.disabled = true;
    if (forwardBtnEl) forwardBtnEl.disabled = true;
  }
}

async function runNavigationCommand(command: string) {
  try {
    setStatus("Loading...");
    const snapshot = await invoke<RenderSnapshot>(command);
    renderSnapshot(snapshot);
    await refreshNavigationState();
    setStatus("Done.");
  } catch (e) {
    setStatus(`Error: ${e}`);
  }
}

async function openCurrentUrl() {
  if (!urlInputEl) {
    return;
  }

  const url = urlInputEl.value.trim();
  if (!url) {
    setStatus("Please enter a URL.");
    return;
  }

  try {
    setStatus("Loading...");
    const snapshot = await invoke<RenderSnapshot>("open_url", { url });
    renderSnapshot(snapshot);
    await refreshNavigationState();
    setStatus("Done.");
  } catch (e) {
    setStatus(`Error: ${e}`);
  }
}

async function loadInitialSnapshot() {
  try {
    const snapshot = await invoke<RenderSnapshot>("get_render_snapshot");
    renderSnapshot(snapshot);
    await refreshNavigationState();
  } catch {
    setStatus("Failed to load initial snapshot.");
  }
}

window.addEventListener("DOMContentLoaded", () => {
  urlInputEl = document.querySelector("#url-input");
  statusEl = document.querySelector("#status");
  currentUrlEl = document.querySelector("#current-url");
  textListEl = document.querySelector("#text-list");
  backBtnEl = document.querySelector("#back-btn");
  forwardBtnEl = document.querySelector("#forward-btn");

  document.querySelector("#url-form")?.addEventListener("submit", (event) => {
    event.preventDefault();
    openCurrentUrl();
  });

  backBtnEl?.addEventListener("click", () => {
    runNavigationCommand("back");
  });

  forwardBtnEl?.addEventListener("click", () => {
    runNavigationCommand("forward");
  });

  document.querySelector("#reload-btn")?.addEventListener("click", () => {
    runNavigationCommand("reload");
  });

  loadInitialSnapshot();
});
