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
  current_url: string | null;
};

type AppError = {
  code: string;
  message: string;
  retryable: boolean;
};

let urlInputEl: HTMLInputElement | null;
let statusEl: HTMLElement | null;
let currentUrlEl: HTMLElement | null;
let textListEl: HTMLUListElement | null;
let backButtonEl: HTMLButtonElement | null;
let forwardButtonEl: HTMLButtonElement | null;
let reloadButtonEl: HTMLButtonElement | null;

function setStatus(message: string, level: "info" | "error" = "info") {
  if (statusEl) {
    statusEl.textContent = message;
    statusEl.dataset.level = level;
  }
}

function setNavButtons(nav: NavigationState) {
  if (backButtonEl) {
    backButtonEl.disabled = !nav.can_back;
  }

  if (forwardButtonEl) {
    forwardButtonEl.disabled = !nav.can_forward;
  }
}

async function refreshNavigationState() {
  try {
    const nav = await invoke<NavigationState>("get_navigation_state");
    setNavButtons(nav);
  } catch {
    if (backButtonEl) {
      backButtonEl.disabled = true;
    }
    if (forwardButtonEl) {
      forwardButtonEl.disabled = true;
    }
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

function errorLabelByCode(code: string): string {
  switch (code) {
    case "network_error":
      return "Network error";
    case "parse_error":
      return "Parse error";
    case "validation_error":
      return "Validation error";
    case "invalid_state":
      return "State error";
    default:
      return "Error";
  }
}

function formatError(e: unknown): string {
  const error = e as Partial<AppError>;
  const code = error.code ?? "unknown_error";
  const message = error.message ?? "Unknown error";
  const retry = error.retryable ? " (retryable)" : "";
  const label = errorLabelByCode(code);

  return `${label} [${code}]: ${message}${retry}`;
}

async function executeNavigationCommand(command: string, loadingMessage: string) {
  try {
    setStatus(loadingMessage);
    const snapshot = await invoke<RenderSnapshot>(command);
    renderSnapshot(snapshot);
    await refreshNavigationState();
    setStatus("Done.");
  } catch (e) {
    setStatus(formatError(e), "error");
  }
}

async function openCurrentUrl() {
  if (!urlInputEl) {
    return;
  }

  const url = urlInputEl.value.trim();
  if (!url) {
    setStatus("Please enter a URL.", "error");
    return;
  }

  try {
    setStatus("Loading...");
    const snapshot = await invoke<RenderSnapshot>("open_url", { url });
    renderSnapshot(snapshot);
    await refreshNavigationState();
    setStatus("Done.");
  } catch (e) {
    setStatus(formatError(e), "error");
  }
}

async function loadInitialSnapshot() {
  try {
    const snapshot = await invoke<RenderSnapshot>("get_render_snapshot");
    renderSnapshot(snapshot);
  } catch {
    setStatus("Failed to load initial snapshot.", "error");
  }

  await refreshNavigationState();
}

window.addEventListener("DOMContentLoaded", () => {
  urlInputEl = document.querySelector("#url-input");
  statusEl = document.querySelector("#status");
  currentUrlEl = document.querySelector("#current-url");
  textListEl = document.querySelector("#text-list");
  backButtonEl = document.querySelector("#back-button");
  forwardButtonEl = document.querySelector("#forward-button");
  reloadButtonEl = document.querySelector("#reload-button");

  document.querySelector("#url-form")?.addEventListener("submit", (event) => {
    event.preventDefault();
    openCurrentUrl();
  });

  backButtonEl?.addEventListener("click", () => {
    executeNavigationCommand("back", "Going back...");
  });

  forwardButtonEl?.addEventListener("click", () => {
    executeNavigationCommand("forward", "Going forward...");
  });

  reloadButtonEl?.addEventListener("click", () => {
    executeNavigationCommand("reload", "Reloading...");
  });

  loadInitialSnapshot();
});
