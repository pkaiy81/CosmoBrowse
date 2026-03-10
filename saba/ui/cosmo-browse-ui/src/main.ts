import { invoke } from "@tauri-apps/api/core";
import { openUrl as openExternalUrl } from "@tauri-apps/plugin-opener";

type FrameRect = { x: number; y: number; width: number; height: number };
type ContentSize = { width: number; height: number };
type FrameViewModel = {
  id: string;
  name: string | null;
  current_url: string;
  title: string;
  diagnostics: string[];
  rect: FrameRect;
  content_size: ContentSize;
  html_content: string | null;
  child_frames: FrameViewModel[];
};
type PageViewModel = {
  current_url: string;
  title: string;
  diagnostics: string[];
  content_size: ContentSize;
  root_frame: FrameViewModel;
};
type NavigationState = { can_back: boolean; can_forward: boolean; current_url: string | null };
type TabSummary = { id: number; title: string; url: string | null; is_active: boolean };
type SearchResult = { id: number; title: string; url: string; snippet: string };
type AppError = { code: string; message: string; retryable: boolean };
type EmbeddedNavigationMessage = {
  type: "cosmobrowse:navigate";
  frameId: string;
  href: string;
  target: string;
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

function isCurrentPageEpoch(epoch: number) {
  return epoch === pageEpoch;
}

function setStatus(message: string, level: "info" | "error" = "info") {
  if (!statusEl) return;
  statusEl.textContent = message;
  statusEl.dataset.level = level;
}

function isUrlLikeInput(input: string) {
  return input.startsWith("http://") || input.startsWith("https://") || input.startsWith("fixture://") || (!input.includes(" ") && input.includes("."));
}

function setNavButtons(nav: NavigationState) {
  if (backButtonEl) backButtonEl.disabled = !nav.can_back;
  if (forwardButtonEl) forwardButtonEl.disabled = !nav.can_forward;
}

function formatError(errorValue: unknown): string {
  const error = errorValue as Partial<AppError>;
  const code = error.code ?? "unknown_error";
  const message = error.message ?? "Unknown error";
  return `${code}: ${message}`;
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
  for (const result of results) {
    const item = document.createElement("li");
    const button = document.createElement("button");
    const detail = document.createElement("small");
    button.type = "button";
    button.className = "linklike";
    button.textContent = result.title;
    button.addEventListener("click", () => void openUrl(result.url));
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
    switchButton.addEventListener("click", () => void switchTab(tab.id));

    const closeButton = document.createElement("button");
    closeButton.type = "button";
    closeButton.className = "tab-close";
    closeButton.textContent = "x";
    closeButton.addEventListener("click", (event) => {
      event.stopPropagation();
      void closeTab(tab.id);
    });

    item.append(switchButton, closeButton);
    tabsListEl.appendChild(item);
  }
}

function renderFrame(frame: FrameViewModel, host: HTMLElement) {
  const shell = document.createElement("section");
  shell.className = frame.child_frames.length > 0 ? "frame-shell frameset-shell" : "frame-shell leaf-shell";
  shell.style.left = `${frame.rect.x}px`;
  shell.style.top = `${frame.rect.y}px`;
  shell.style.width = `${Math.max(frame.rect.width, 1)}px`;
  shell.style.height = `${Math.max(frame.rect.height, 1)}px`;
  shell.dataset.frameId = frame.id;

  if (frame.child_frames.length > 0) {
    const childrenRoot = document.createElement("div");
    childrenRoot.className = "frame-children";
    for (const child of frame.child_frames) {
      renderFrame(child, childrenRoot);
    }
    shell.appendChild(childrenRoot);
  } else {
    const iframe = document.createElement("iframe");
    iframe.className = "frame-document";
    iframe.title = frame.title || frame.current_url;
    iframe.srcdoc = frame.html_content ?? "<html><body></body></html>";
    shell.appendChild(iframe);
  }

  host.appendChild(shell);
}

function renderFrameTree(view: PageViewModel) {
  if (!sceneRootEl) return;
  sceneRootEl.innerHTML = "";
  sceneRootEl.style.width = `${Math.max(view.content_size.width, 320)}px`;
  sceneRootEl.style.height = `${Math.max(view.content_size.height, 240)}px`;
  renderFrame(view.root_frame, sceneRootEl);
}

function renderPageView(view: PageViewModel) {
  if (currentUrlEl) currentUrlEl.textContent = view.current_url ? `URL: ${view.current_url}` : "URL: (none)";
  if (pageTitleEl) pageTitleEl.textContent = view.title || "New Tab";
  if (urlInputEl && view.current_url) urlInputEl.value = view.current_url;
  document.title = view.title ? `${view.title} - CosmoBrowse` : "CosmoBrowse";
  renderFrameTree(view);
  const diagnostics = view.diagnostics.filter((entry) => entry.trim().length > 0);
  setStatus(diagnostics.length > 0 ? diagnostics.join(" | ") : "Done.");
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

async function syncViewport(force = false, epoch = pageEpoch) {
  if (!viewportEl) return;
  const width = Math.floor(viewportEl.clientWidth - 32);
  const height = Math.floor(viewportEl.clientHeight - 32);
  if (width <= 0 || height <= 0) return;

  if (!force && resizeTimer) window.clearTimeout(resizeTimer);
  const run = async () => {
    try {
      const view = await invoke<PageViewModel>("set_viewport", { width, height });
      if (!isCurrentPageEpoch(epoch)) return;
      renderPageView(view);
      await refreshNavigationState();
      await refreshTabs();
    } catch {
      // Ignore transient resize failures during startup.
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
  } catch (errorValue) {
    if (!isCurrentPageEpoch(epoch)) return;
    setStatus(formatError(errorValue), "error");
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
  } catch (errorValue) {
    if (!isCurrentPageEpoch(epoch)) return;
    setStatus(formatError(errorValue), "error");
  }
}

// Ref: HTML Living Standard browsing context keywords are matched using ASCII
// case-insensitive comparisons after attribute value processing.
// https://html.spec.whatwg.org/multipage/browsing-the-web.html#valid-browsing-context-name-or-keyword
function normalizeTargetKeyword(target?: string | null): string | null {
  const normalizedTarget = target?.trim();
  if (!normalizedTarget) return null;
  if (normalizedTarget.startsWith("_")) return normalizedTarget.toLowerCase();
  return normalizedTarget;
}

// Ref: HTML Living Standard browsing context names and keywords.
// https://html.spec.whatwg.org/multipage/browsing-the-web.html#valid-browsing-context-name-or-keyword
async function openExternalNavigation(href: string) {
  await openExternalUrl(href);
}

async function handleEmbeddedNavigation(message: EmbeddedNavigationMessage) {
  if (!message.href) return;
  const normalizedTarget = normalizeTargetKeyword(message.target);
  if (message.href.startsWith("mailto:") || normalizedTarget === "_blank") {
    try {
      await openExternalNavigation(message.href);
      setStatus(`Opened externally: ${message.href}`);
    } catch (errorValue) {
      setStatus(formatError(errorValue), "error");
    }
    return;
  }

  const epoch = beginPageEpoch();
  try {
    setStatus("Navigating...");
    const view = await invoke<PageViewModel>("activate_link", {
      frameId: message.frameId,
      href: message.href,
      target: normalizedTarget,
    });
    if (!isCurrentPageEpoch(epoch)) return;
    renderPageView(view);
    await refreshNavigationState();
    await refreshTabs();
  } catch (errorValue) {
    if (!isCurrentPageEpoch(epoch)) return;
    setStatus(formatError(errorValue), "error");
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
  } catch (errorValue) {
    if (!isCurrentPageEpoch(epoch)) return;
    setStatus(formatError(errorValue), "error");
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
  } catch (errorValue) {
    if (!isCurrentPageEpoch(epoch)) return;
    setStatus(formatError(errorValue), "error");
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
  } catch (errorValue) {
    if (!isCurrentPageEpoch(epoch)) return;
    setStatus(formatError(errorValue), "error");
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

window.addEventListener("message", (event) => {
  const data = event.data as EmbeddedNavigationMessage | undefined;
  if (!data || data.type !== "cosmobrowse:navigate") return;
  void handleEmbeddedNavigation(data);
});

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
  backButtonEl?.addEventListener("click", () => void executeNavigationCommand("back", "Going back..."));
  forwardButtonEl?.addEventListener("click", () => void executeNavigationCommand("forward", "Going forward..."));
  reloadButtonEl?.addEventListener("click", () => void executeNavigationCommand("reload", "Reloading..."));
  newTabButtonEl?.addEventListener("click", () => void createTab());

  if (viewportEl) {
    resizeObserver = new ResizeObserver(() => void syncViewport());
    resizeObserver.observe(viewportEl);
  }

  void loadInitialPageView();
});


