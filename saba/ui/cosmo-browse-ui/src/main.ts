import { invoke } from "@tauri-apps/api/core";

type SceneRect = {
  kind: "rect";
  x: number;
  y: number;
  width: number;
  height: number;
  background_color: string;
};

type SceneText = {
  kind: "text";
  x: number;
  y: number;
  text: string;
  color: string;
  font_px: number;
  underline: boolean;
  href: string | null;
};

type SceneItem = SceneRect | SceneText;

type ContentSize = {
  width: number;
  height: number;
};

type PageViewModel = {
  current_url: string;
  title: string;
  diagnostics: string[];
  content_size: ContentSize;
  scene_items: SceneItem[];
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
let pageTitleEl: HTMLElement | null;
let backButtonEl: HTMLButtonElement | null;
let forwardButtonEl: HTMLButtonElement | null;
let reloadButtonEl: HTMLButtonElement | null;
let tabsListEl: HTMLUListElement | null;
let newTabButtonEl: HTMLButtonElement | null;
let searchResultsEl: HTMLUListElement | null;
let viewportEl: HTMLElement | null;
let sceneRootEl: HTMLElement | null;
let resizeObserver: ResizeObserver | null = null;
let resizeTimer: number | null = null;
let pageEpoch = 0;

function beginPageEpoch() {
  pageEpoch += 1;
  return pageEpoch;
}

function currentPageEpoch() {
  return pageEpoch;
}

function isCurrentPageEpoch(epoch: number) {
  return epoch === pageEpoch;
}

function setStatus(message: string, level: "info" | "error" = "info") {
  if (!statusEl) return;
  statusEl.textContent = message;
  statusEl.dataset.level = level;
}

function isUrlLikeInput(input: string) {
  return (
    input.startsWith("http://") ||
    input.startsWith("https://") ||
    (!input.includes(" ") && input.includes("."))
  );
}

function setNavButtons(nav: NavigationState) {
  if (backButtonEl) backButtonEl.disabled = !nav.can_back;
  if (forwardButtonEl) forwardButtonEl.disabled = !nav.can_forward;
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

function clearSearchResults() {
  if (!searchResultsEl) return;
  searchResultsEl.innerHTML = "";
  searchResultsEl.classList.add("hidden");
}

function renderSearchResults(results: SearchResult[]) {
  if (!searchResultsEl) return;

  searchResultsEl.innerHTML = "";
  searchResultsEl.classList.toggle("hidden", results.length === 0);

  if (results.length === 0) {
    return;
  }

  for (const result of results) {
    const item = document.createElement("li");
    const button = document.createElement("button");
    const detail = document.createElement("small");

    button.type = "button";
    button.className = "linklike";
    button.textContent = result.title;
    button.addEventListener("click", () => {
      clearSearchResults();
      openUrl(result.url);
    });

    detail.textContent = `${result.url} - ${result.snippet}`;
    item.append(button, document.createElement("br"), detail);
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
    switchButton.textContent = tab.title || tab.url || "New Tab";
    switchButton.addEventListener("click", () => switchTab(tab.id));

    const closeButton = document.createElement("button");
    closeButton.type = "button";
    closeButton.className = "tab-close";
    closeButton.textContent = "x";
    closeButton.addEventListener("click", (event) => {
      event.stopPropagation();
      closeTab(tab.id);
    });

    item.append(switchButton, closeButton);
    tabsListEl.appendChild(item);
  }
}

function renderScene(sceneItems: SceneItem[], contentSize: ContentSize) {
  if (!sceneRootEl) return;
  sceneRootEl.innerHTML = "";
  sceneRootEl.style.width = `${Math.max(contentSize.width, 320)}px`;
  sceneRootEl.style.height = `${Math.max(contentSize.height, 240)}px`;

  for (const item of sceneItems) {
    if (item.kind === "rect") {
      const rect = document.createElement("div");
      rect.className = "scene-rect";
      rect.style.left = `${item.x}px`;
      rect.style.top = `${item.y}px`;
      rect.style.width = `${item.width}px`;
      rect.style.height = `${item.height}px`;
      rect.style.backgroundColor = item.background_color;
      sceneRootEl.appendChild(rect);
      continue;
    }

    const textNode = document.createElement(item.href ? "button" : "div");
    textNode.className = item.href ? "scene-link" : "scene-text";
    textNode.textContent = item.text;
    textNode.style.left = `${item.x}px`;
    textNode.style.top = `${item.y}px`;
    textNode.style.color = item.color;
    textNode.style.fontSize = `${item.font_px}px`;
    textNode.style.textDecoration = item.underline ? "underline" : "none";

    if (item.href) {
      (textNode as HTMLButtonElement).type = "button";
      textNode.addEventListener("click", () => openUrl(item.href ?? ""));
    }

    sceneRootEl.appendChild(textNode);
  }
}

function renderPageView(view: PageViewModel) {
  if (currentUrlEl) {
    currentUrlEl.textContent = view.current_url ? `URL: ${view.current_url}` : "URL: (none)";
  }

  if (pageTitleEl) {
    pageTitleEl.textContent = view.title || "New Tab";
  }

  document.title = view.title ? `${view.title} - CosmoBrowse` : "CosmoBrowse";

  if (urlInputEl && view.current_url) {
    urlInputEl.value = view.current_url;
  }

  renderScene(view.scene_items, view.content_size);

  const diagnostics = view.diagnostics.filter((entry) => entry.trim().length > 0);
  if (diagnostics.length > 0) {
    setStatus(diagnostics.join(" | "));
  }
}

async function refreshTabs() {
  const tabs = await invoke<TabSummary[]>("list_tabs");
  renderTabs(tabs);
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

async function syncViewport(force = false, epoch = currentPageEpoch()) {
  if (!viewportEl) return;

  const width = Math.floor(viewportEl.clientWidth - 32);
  const height = Math.floor(viewportEl.clientHeight - 32);
  if (width <= 0 || height <= 0) return;

  if (!force && resizeTimer) {
    window.clearTimeout(resizeTimer);
  }

  const run = async () => {
    try {
      const view = await invoke<PageViewModel>("set_viewport", { width, height });
      if (!isCurrentPageEpoch(epoch)) return;
      renderPageView(view);
      await refreshNavigationState();
      await refreshTabs();
    } catch {
      // Ignore transient resize errors while the app initializes.
    }
  };

  if (force) {
    await run();
  } else {
    resizeTimer = window.setTimeout(run, 120);
  }
}

async function executeNavigationCommand(command: string, loadingMessage: string) {
  const epoch = beginPageEpoch();
  try {
    clearSearchResults();
    setStatus(loadingMessage);
    const view = await invoke<PageViewModel>(command);
    if (!isCurrentPageEpoch(epoch)) return;
    renderPageView(view);
    await refreshNavigationState();
    await refreshTabs();
    if (!isCurrentPageEpoch(epoch)) return;
    setStatus("Done.");
  } catch (e) {
    if (!isCurrentPageEpoch(epoch)) return;
    setStatus(formatError(e), "error");
  }
}

async function openUrl(url: string) {
  const epoch = beginPageEpoch();
  try {
    clearSearchResults();
    setStatus("Loading...");
    const view = await invoke<PageViewModel>("open_url", { url });
    if (!isCurrentPageEpoch(epoch)) return;
    renderPageView(view);
    await refreshNavigationState();
    await refreshTabs();
    if (!isCurrentPageEpoch(epoch)) return;
    setStatus("Done.");
  } catch (e) {
    if (!isCurrentPageEpoch(epoch)) return;
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
  } catch (errorValue) {
    setStatus(formatError(errorValue), "error");
  }
}

async function switchTab(id: number) {
  const epoch = beginPageEpoch();
  try {
    clearSearchResults();
    const view = await invoke<PageViewModel>("switch_tab", { id });
    if (!isCurrentPageEpoch(epoch)) return;
    renderPageView(view);
    await refreshNavigationState();
    await refreshTabs();
  } catch (e) {
    if (!isCurrentPageEpoch(epoch)) return;
    setStatus(formatError(e), "error");
  }
}

async function closeTab(id: number) {
  const epoch = beginPageEpoch();
  try {
    clearSearchResults();
    await invoke<TabSummary[]>("close_tab", { id });
    if (!isCurrentPageEpoch(epoch)) return;
    const view = await invoke<PageViewModel>("get_page_view");
    if (!isCurrentPageEpoch(epoch)) return;
    renderPageView(view);
    await refreshNavigationState();
    await refreshTabs();
  } catch (e) {
    if (!isCurrentPageEpoch(epoch)) return;
    setStatus(formatError(e), "error");
  }
}

async function createTab() {
  const epoch = beginPageEpoch();
  try {
    clearSearchResults();
    await invoke<TabSummary>("new_tab");
    if (!isCurrentPageEpoch(epoch)) return;
    const view = await invoke<PageViewModel>("get_page_view");
    if (!isCurrentPageEpoch(epoch)) return;
    renderPageView(view);
    await refreshNavigationState();
    await refreshTabs();
  } catch (e) {
    if (!isCurrentPageEpoch(epoch)) return;
    setStatus(formatError(e), "error");
  }
}

async function loadInitialPageView() {
  const epoch = beginPageEpoch();
  try {
    await syncViewport(true, epoch);
    if (!isCurrentPageEpoch(epoch)) return;
    const view = await invoke<PageViewModel>("get_page_view");
    if (!isCurrentPageEpoch(epoch)) return;
    renderPageView(view);
    await refreshNavigationState();
    await refreshTabs();
  } catch {
    if (!isCurrentPageEpoch(epoch)) return;
    setStatus("Failed to load initial page.", "error");
  }
}

window.addEventListener("DOMContentLoaded", () => {
  urlInputEl = document.querySelector("#url-input");
  statusEl = document.querySelector("#status");
  currentUrlEl = document.querySelector("#current-url");
  pageTitleEl = document.querySelector("#page-title");
  backButtonEl = document.querySelector("#back-button");
  forwardButtonEl = document.querySelector("#forward-button");
  reloadButtonEl = document.querySelector("#reload-button");
  tabsListEl = document.querySelector("#tabs-list");
  newTabButtonEl = document.querySelector("#new-tab-button");
  searchResultsEl = document.querySelector("#search-results");
  viewportEl = document.querySelector("#viewport");
  sceneRootEl = document.querySelector("#scene-root");

  document.querySelector("#url-form")?.addEventListener("submit", (event) => {
    event.preventDefault();
    void openCurrentInput();
  });

  backButtonEl?.addEventListener("click", () => {
    void executeNavigationCommand("back", "Going back...");
  });

  forwardButtonEl?.addEventListener("click", () => {
    void executeNavigationCommand("forward", "Going forward...");
  });

  reloadButtonEl?.addEventListener("click", () => {
    void executeNavigationCommand("reload", "Reloading...");
  });

  newTabButtonEl?.addEventListener("click", () => {
    void createTab();
  });

  if (viewportEl) {
    resizeObserver = new ResizeObserver(() => {
      void syncViewport();
    });
    resizeObserver.observe(viewportEl);
  }

  void loadInitialPageView();
});
