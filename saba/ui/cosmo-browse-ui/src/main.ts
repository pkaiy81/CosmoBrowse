import { invoke } from "@tauri-apps/api/core";

type SnapshotLink = {
  text: string;
  href: string;
};

type LayoutMetadata = {
  text_item_count: number;
  block_item_count: number;
  content_width: number;
  content_height: number;
};

type StyleMetadata = {
  text_colors: string[];
  background_colors: string[];
  underlined_text_count: number;
  heading_text_count: number;
};

type RenderSnapshot = {
  current_url: string;
  title: string;
  text_blocks: string[];
  links: SnapshotLink[];
  diagnostics: string[];
  layout: LayoutMetadata;
  style: StyleMetadata;
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

type ErrorMetric = {
  code: string;
  count: number;
};

type NavigationEvent = {
  command: string;
  url: string | null;
  duration_ms: number;
  success: boolean;
  error_code: string | null;
  recorded_at_ms: number;
};

type AppMetricsSnapshot = {
  total_navigations: number;
  successful_navigations: number;
  failed_navigations: number;
  average_duration_ms: number;
  last_duration_ms: number | null;
  error_counts: ErrorMetric[];
  recent_events: NavigationEvent[];
};

let urlInputEl: HTMLInputElement | null;
let statusEl: HTMLElement | null;
let currentUrlEl: HTMLElement | null;
let textListEl: HTMLUListElement | null;
let snapshotMetaEl: HTMLUListElement | null;
let diagnosticsListEl: HTMLUListElement | null;
let linksListEl: HTMLUListElement | null;
let metricsSummaryEl: HTMLUListElement | null;
let metricsEventsEl: HTMLUListElement | null;
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

function setTextItems(list: HTMLUListElement | null, lines: string[], emptyLabel: string) {
  if (!list) return;

  list.innerHTML = "";
  const items = lines.length > 0 ? lines : [emptyLabel];

  for (const line of items) {
    const item = document.createElement("li");
    item.textContent = line;
    list.appendChild(item);
  }
}

function renderSnapshotMeta(snapshot: RenderSnapshot) {
  const styleSummary = [
    `Text colors: ${snapshot.style.text_colors.join(", ") || "(none)"}`,
    `Background colors: ${snapshot.style.background_colors.join(", ") || "(none)"}`,
    `Underlined text items: ${snapshot.style.underlined_text_count}`,
    `Heading text items: ${snapshot.style.heading_text_count}`,
  ];

  const layoutSummary = [
    `Text items: ${snapshot.layout.text_item_count}`,
    `Block items: ${snapshot.layout.block_item_count}`,
    `Content width: ${snapshot.layout.content_width}`,
    `Content height: ${snapshot.layout.content_height}`,
  ];

  setTextItems(snapshotMetaEl, [...layoutSummary, ...styleSummary], "No snapshot metadata.");
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

  renderSnapshotMeta(snapshot);
  setTextItems(textListEl, snapshot.text_blocks, "No content.");
  setTextItems(
    diagnosticsListEl,
    snapshot.diagnostics,
    "No diagnostics."
  );
  renderLinks(snapshot.links);
}

function renderLinks(links: SnapshotLink[]) {
  if (!linksListEl) return;

  linksListEl.innerHTML = "";

  if (links.length === 0) {
    setTextItems(linksListEl, [], "No links found.");
    return;
  }

  for (const link of links) {
    const item = document.createElement("li");
    const button = document.createElement("button");
    const detail = document.createElement("small");

    button.type = "button";
    button.className = "linklike";
    button.textContent = link.text || link.href;
    button.addEventListener("click", () => openUrl(link.href));

    detail.textContent = link.href;

    item.appendChild(button);
    item.appendChild(document.createElement("br"));
    item.appendChild(detail);
    linksListEl.appendChild(item);
  }
}

function renderSearchResults(results: SearchResult[]) {
  if (!searchResultsEl) return;

  searchResultsEl.innerHTML = "";

  if (results.length === 0) {
    setTextItems(searchResultsEl, [], "No search results.");
    return;
  }

  for (const result of results) {
    const item = document.createElement("li");
    const button = document.createElement("button");
    const detail = document.createElement("small");

    button.type = "button";
    button.className = "linklike";
    button.textContent = result.title;
    button.addEventListener("click", () => openUrl(result.url));

    detail.textContent = `${result.url} | ${result.snippet}`;

    item.appendChild(button);
    item.appendChild(document.createElement("br"));
    item.appendChild(detail);
    searchResultsEl.appendChild(item);
  }
}

function renderTabs(tabs: TabSummary[]) {
  if (!tabsListEl) return;

  tabsListEl.innerHTML = "";

  if (tabs.length === 0) {
    setTextItems(tabsListEl, [], "No tabs.");
    return;
  }

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
    closeButton.textContent = "x";
    closeButton.addEventListener("click", (event) => {
      event.stopPropagation();
      closeTab(tab.id);
    });

    item.appendChild(switchButton);
    item.appendChild(closeButton);
    tabsListEl.appendChild(item);
  }
}

function renderMetrics(metrics: AppMetricsSnapshot) {
  const summaryLines = [
    `Total navigations: ${metrics.total_navigations}`,
    `Successful navigations: ${metrics.successful_navigations}`,
    `Failed navigations: ${metrics.failed_navigations}`,
    `Average duration: ${metrics.average_duration_ms} ms`,
    `Last duration: ${metrics.last_duration_ms ?? 0} ms`,
    `Error counts: ${formatErrorCounts(metrics.error_counts)}`,
  ];

  setTextItems(metricsSummaryEl, summaryLines, "No metrics.");

  const eventLines = metrics.recent_events
    .slice()
    .reverse()
    .map((event) => {
      const time = new Date(event.recorded_at_ms).toLocaleTimeString();
      const status = event.success ? "ok" : `failed (${event.error_code ?? "unknown"})`;
      const target = event.url ?? "(no url)";
      return `${time} | ${event.command} | ${status} | ${event.duration_ms} ms | ${target}`;
    });

  setTextItems(metricsEventsEl, eventLines, "No recent events.");
}

function formatErrorCounts(errorCounts: ErrorMetric[]) {
  if (errorCounts.length === 0) {
    return "none";
  }

  return errorCounts.map((metric) => `${metric.code}: ${metric.count}`).join(", ");
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

async function refreshTabs() {
  const tabs = await invoke<TabSummary[]>("list_tabs");
  renderTabs(tabs);
}

async function refreshMetrics() {
  try {
    const metrics = await invoke<AppMetricsSnapshot>("get_metrics");
    renderMetrics(metrics);
  } catch {
    setTextItems(metricsSummaryEl, [], "Failed to load metrics.");
    setTextItems(metricsEventsEl, [], "Failed to load recent events.");
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

function formatError(errorValue: unknown): string {
  const error = errorValue as Partial<AppError>;
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
    renderSearchResults([]);
    await Promise.all([refreshNavigationState(), refreshTabs(), refreshMetrics()]);
    setStatus("Done.");
  } catch (errorValue) {
    await refreshMetrics();
    setStatus(formatError(errorValue), "error");
  }
}

async function openUrl(url: string) {
  try {
    setStatus("Loading...");
    const snapshot = await invoke<RenderSnapshot>("open_url", { url });
    renderSnapshot(snapshot);
    renderSearchResults([]);
    await Promise.all([refreshNavigationState(), refreshTabs(), refreshMetrics()]);
    setStatus("Done.");
  } catch (errorValue) {
    await refreshMetrics();
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
    setStatus("Searching...");
    const results = await invoke<SearchResult[]>("search", { query: value });
    renderSearchResults(results);
    await refreshMetrics();
    setStatus(`Found ${results.length} search results.`);
  } catch (errorValue) {
    setStatus(formatError(errorValue), "error");
  }
}

async function switchTab(id: number) {
  try {
    const snapshot = await invoke<RenderSnapshot>("switch_tab", { id });
    renderSnapshot(snapshot);
    await Promise.all([refreshNavigationState(), refreshTabs(), refreshMetrics()]);
  } catch (errorValue) {
    setStatus(formatError(errorValue), "error");
  }
}

async function closeTab(id: number) {
  try {
    await invoke<TabSummary[]>("close_tab", { id });
    const snapshot = await invoke<RenderSnapshot>("get_render_snapshot");
    renderSnapshot(snapshot);
    await Promise.all([refreshNavigationState(), refreshTabs(), refreshMetrics()]);
  } catch (errorValue) {
    setStatus(formatError(errorValue), "error");
  }
}

async function createTab() {
  try {
    await invoke<TabSummary>("new_tab");
    const snapshot = await invoke<RenderSnapshot>("get_render_snapshot");
    renderSnapshot(snapshot);
    await Promise.all([refreshNavigationState(), refreshTabs(), refreshMetrics()]);
  } catch (errorValue) {
    setStatus(formatError(errorValue), "error");
  }
}

async function loadInitialSnapshot() {
  try {
    const snapshot = await invoke<RenderSnapshot>("get_render_snapshot");
    renderSnapshot(snapshot);
  } catch {
    setStatus("Failed to load initial snapshot.", "error");
  }

  renderSearchResults([]);
  await Promise.all([refreshTabs(), refreshNavigationState(), refreshMetrics()]);
}

window.addEventListener("DOMContentLoaded", () => {
  urlInputEl = document.querySelector("#url-input");
  statusEl = document.querySelector("#status");
  currentUrlEl = document.querySelector("#current-url");
  textListEl = document.querySelector("#text-list");
  snapshotMetaEl = document.querySelector("#snapshot-meta");
  diagnosticsListEl = document.querySelector("#diagnostics-list");
  linksListEl = document.querySelector("#links-list");
  metricsSummaryEl = document.querySelector("#metrics-summary");
  metricsEventsEl = document.querySelector("#metrics-events");
  backButtonEl = document.querySelector("#back-button");
  forwardButtonEl = document.querySelector("#forward-button");
  reloadButtonEl = document.querySelector("#reload-button");
  tabsListEl = document.querySelector("#tabs-list");
  newTabButtonEl = document.querySelector("#new-tab-button");
  searchResultsEl = document.querySelector("#search-results");

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

  void loadInitialSnapshot();
});
