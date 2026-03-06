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

type TabSummary = {
  id: number;
  title: string;
  url: string | null;
  is_active: boolean;
};

type SearchResult = {
  id: number;
  title: string;
  url: string;
  snippet: string;
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
let tabsListEl: HTMLUListElement | null;
let newTabButtonEl: HTMLButtonElement | null;
let searchResultsEl: HTMLUListElement | null;

function setStatus(message: string, level: "info" | "error" = "info") {
  if (statusEl) {
    statusEl.textContent = message;
    statusEl.dataset.level = level;
  }
}

function isUrlLikeInput(input: string) {
  return (
    input.startsWith("http://") ||
    input.startsWith("https://") ||
    (!input.includes(" ") && input.includes("."))
  );
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
    if (backButtonEl) backButtonEl.disabled = true;
    if (forwardButtonEl) forwardButtonEl.disabled = true;
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

  if (!textListEl) return;

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

function renderSearchResults(results: SearchResult[]) {
  if (!searchResultsEl) return;

  searchResultsEl.innerHTML = "";

  for (const result of results) {
    const item = document.createElement("li");
    const button = document.createElement("button");
    const detail = document.createElement("small");

    button.type = "button";
    button.className = "linklike";
    button.textContent = result.title;
    button.addEventListener("click", () => openUrl(result.url));

    detail.textContent = `${result.url} — ${result.snippet}`;

    item.appendChild(button);
    item.appendChild(document.createElement("br"));
    item.appendChild(detail);
    searchResultsEl.appendChild(item);
  }
}

function renderTabs(tabs: TabSummary[]) {
  if (!tabsListEl) return;

  tabsListEl.innerHTML = "";

  for (const tab of tabs) {
    const item = document.createElement("li");
    item.className = tab.is_active ? "tab-item active" : "tab-item";

    const switchButton = document.createElement("button");
    switchButton.type = "button";
    switchButton.className = "tab-switch";
    switchButton.textContent = tab.url ?? tab.title;
    switchButton.addEventListener("click", () => switchTab(tab.id));

    const closeButton = document.createElement("button");
    closeButton.type = "button";
    closeButton.className = "tab-close";
    closeButton.textContent = "×";
    closeButton.addEventListener("click", (event) => {
      event.stopPropagation();
      closeTab(tab.id);
    });

    item.appendChild(switchButton);
    item.appendChild(closeButton);
    tabsListEl.appendChild(item);
  }
}

async function refreshTabs() {
  const tabs = await invoke<TabSummary[]>("list_tabs");
  renderTabs(tabs);
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
    await refreshTabs();
    setStatus("Done.");
  } catch (e) {
    setStatus(formatError(e), "error");
  }
}

async function openUrl(url: string) {
  try {
    setStatus("Loading...");
    const snapshot = await invoke<RenderSnapshot>("open_url", { url });
    renderSnapshot(snapshot);
    renderSearchResults([]);
    await refreshNavigationState();
    await refreshTabs();
    setStatus("Done.");
  } catch (e) {
    setStatus(formatError(e), "error");
  }
}

async function openCurrentInput() {
  if (!urlInputEl) return;

  const value = urlInputEl.value.trim();
  if (!value) {
    setStatus("Please enter a URL or search query.", "error");
    return;
  }

  if (isUrlLikeInput(value)) {
    await openUrl(value);
    return;
  }

  try {
    setStatus("Searching...");
    const results = await invoke<SearchResult[]>("search", { query: value });
    renderSearchResults(results);
    setStatus(`Found ${results.length} search results.`);
  } catch (e) {
    setStatus(formatError(e), "error");
  }
}

async function switchTab(id: number) {
  try {
    const snapshot = await invoke<RenderSnapshot>("switch_tab", { id });
    renderSnapshot(snapshot);
    await refreshNavigationState();
    await refreshTabs();
  } catch (e) {
    setStatus(formatError(e), "error");
  }
}

async function closeTab(id: number) {
  try {
    await invoke<TabSummary[]>("close_tab", { id });
    await refreshTabs();
    const snapshot = await invoke<RenderSnapshot>("get_render_snapshot");
    renderSnapshot(snapshot);
    await refreshNavigationState();
  } catch (e) {
    setStatus(formatError(e), "error");
  }
}

async function createTab() {
  try {
    await invoke<TabSummary>("new_tab");
    await refreshTabs();
    const snapshot = await invoke<RenderSnapshot>("get_render_snapshot");
    renderSnapshot(snapshot);
    await refreshNavigationState();
  } catch (e) {
    setStatus(formatError(e), "error");
  }
}

async function loadInitialSnapshot() {
  try {
    const snapshot = await invoke<RenderSnapshot>("get_render_snapshot");
    renderSnapshot(snapshot);
    await refreshTabs();
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
  tabsListEl = document.querySelector("#tabs-list");
  newTabButtonEl = document.querySelector("#new-tab-button");
  searchResultsEl = document.querySelector("#search-results");

  document.querySelector("#url-form")?.addEventListener("submit", (event) => {
    event.preventDefault();
    openCurrentInput();
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

  newTabButtonEl?.addEventListener("click", () => {
    createTab();
  });

  loadInitialSnapshot();
});
