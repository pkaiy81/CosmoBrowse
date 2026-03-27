import { invoke } from "@tauri-apps/api/core";
import { openUrl as openExternalUrl } from "@tauri-apps/plugin-opener";

type FrameRect = { x: number; y: number; width: number; height: number };
type ContentSize = { width: number; height: number };
type ScrollPosition = { x: number; y: number };
type RenderTreeSnapshot = { root: RenderNode | null };
type RenderNodeKind = "block" | "inline" | "text";
type RenderNode = {
  kind: RenderNodeKind;
  node_name: string;
  text: string | null;
  box_info: FrameRect;
  style: {
    display: string;
    position: string;
    color: string;
    background_color: string;
    font_px: number;
    font_family: string;
    opacity: number;
    z_index: number;
  };
  children: RenderNode[];
};
type FrameViewModel = {
  id: string;
  name: string | null;
  current_url: string;
  title: string;
  diagnostics: string[];
  rect: FrameRect;
  content_size: ContentSize;
  scroll_position: ScrollPosition;
  render_backend: "web_view" | "native_scene";
  document_url: string;
  scene_items: SceneItem[];
  paint_commands: PaintCommand[];
  render_tree: RenderTreeSnapshot | null;
  html_content: string | null;
  child_frames: FrameViewModel[];
};
type SceneItemRect = {
  kind: "rect";
  x: number;
  y: number;
  width: number;
  height: number;
  background_color: string;
  opacity: number;
};
type SceneItemText = {
  kind: "text";
  x: number;
  y: number;
  text: string;
  color: string;
  font_px: number;
  font_family: string;
  underline: boolean;
  opacity: number;
  href: string | null;
  target: string | null;
};
type SceneItemImage = {
  kind: "image";
  x: number;
  y: number;
  width: number;
  height: number;
  src: string;
  alt: string;
  opacity: number;
  href: string | null;
  target: string | null;
};
type SceneItem = SceneItemRect | SceneItemText | SceneItemImage;

type DrawRectCommand = {
  kind: "draw_rect";
  x: number;
  y: number;
  width: number;
  height: number;
  background_color: string;
  opacity: number;
  z_index: number;
  clip_rect: [number, number, number, number] | null;
};
type DrawTextCommand = {
  kind: "draw_text";
  x: number;
  y: number;
  text: string;
  color: string;
  font_px: number;
  font_family: string;
  underline: boolean;
  opacity: number;
  href: string | null;
  target: string | null;
  z_index: number;
  clip_rect: [number, number, number, number] | null;
};
type DrawImageCommand = {
  kind: "draw_image";
  x: number;
  y: number;
  width: number;
  height: number;
  src: string;
  alt: string;
  opacity: number;
  href: string | null;
  target: string | null;
  z_index: number;
  clip_rect: [number, number, number, number] | null;
};
type PaintCommand = DrawRectCommand | DrawTextCommand | DrawImageCommand;
type DomSnapshotEntry = { frame_id: string; document_url: string; html: string };
type PageViewModel = {
  current_url: string;
  title: string;
  diagnostics: string[];
  content_size: ContentSize;
  network_log: string[];
  console_log: string[];
  dom_snapshot: DomSnapshotEntry[];
  root_frame: FrameViewModel;
};
type NavigationType = "document" | "hash" | "redirect";
type NavigationState = { can_back: boolean; can_forward: boolean; current_url: string | null; current_navigation_type?: NavigationType | null };
type TabSummary = {
  id: number;
  title: string;
  url: string | null;
  is_active: boolean;
  is_pinned: boolean;
  is_muted: boolean;
};
type SearchResult = { id: number; title: string; url: string; snippet: string };
type OmniboxSuggestionKind = "history" | "search" | "url";
type OmniboxSuggestion = {
  id: number;
  kind: OmniboxSuggestionKind;
  title: string;
  value: string;
  url: string | null;
  detail: string;
};
type OmniboxSuggestionSet = {
  query: string;
  active_index: number | null;
  suggestions: OmniboxSuggestion[];
};
type FrameScrollPositionSnapshot = { frame_id: string; position: ScrollPosition };
type AppError = { code: string; message: string; retryable: boolean };
type DownloadState = "queued" | "downloading" | "paused" | "completed" | "cancelled" | "failed";
type DownloadSavePolicy = {
  directory: string;
  conflict_policy: string;
  requires_user_confirmation: boolean;
};
type DownloadSitePolicy = {
  origin: string;
  policy: DownloadSavePolicy;
};
type DownloadPolicySettings = {
  default_policy: DownloadSavePolicy;
  site_policies: DownloadSitePolicy[];
};
type DownloadEntry = {
  id: number;
  url: string;
  final_url: string | null;
  file_name: string;
  save_path: string;
  total_bytes: number | null;
  downloaded_bytes: number;
  state: DownloadState;
  supports_resume: boolean | null;
  save_policy: DownloadSavePolicy;
  last_error: AppError | null;
  status_message: string | null;
  created_at_ms: number;
  updated_at_ms: number;
};
type EmbeddedNavigationMessage = {
  type: "cosmobrowse:navigate";
  frameId: string;
  href: string;
  target: string;
};

type ErrorContext = {
  appError: AppError;
  requestedUrl?: string;
};

type IpcRequest = {
  version: number;
  type: string;
  payload?: Record<string, unknown>;
};

type IpcEnvelope<T> = {
  version: number;
  type: string;
  payload: T;
};

type ReleaseRolloutStatus = {
  channel: string;
  native_default_enabled: boolean;
  adapter_tauri_fallback_enabled: boolean;
  rollout_percentage: number;
  assignment_bucket: number;
  assigned_transport: "adapter_native" | "adapter_tauri";
};

type TransportKind = "adapter_native" | "adapter_tauri";

type AppTransport = {
  kind: TransportKind;
  request: <T>(request: Omit<IpcRequest, "version">) => Promise<T>;
};

const IPC_SCHEMA_VERSION = 1;
const TRANSPORT_OVERRIDE_KEY = "cosmobrowse.transport.override";

async function invokeNativeIpc<T>(request: Omit<IpcRequest, "version">): Promise<T> {
  const response = await invoke<IpcEnvelope<T>>("dispatch_ipc", {
    request: { version: IPC_SCHEMA_VERSION, ...request },
  });
  if (response.version !== IPC_SCHEMA_VERSION) {
    throw new Error(`Unsupported IPC response version: ${response.version}`);
  }
  return response.payload;
}

function legacyInvoke<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  return invoke<T>(command, args);
}

const legacyTransportHandlers: Record<string, (payload?: Record<string, unknown>) => Promise<unknown>> = {
  open_url: (payload) => legacyInvoke<PageViewModel>("open_url", { url: `${payload?.url ?? ""}` }),
  activate_link: (payload) =>
    legacyInvoke<PageViewModel>("activate_link", {
      frameId: `${payload?.frame_id ?? ""}`,
      href: `${payload?.href ?? ""}`,
      target: typeof payload?.target === "string" ? payload.target : null,
    }),
  get_page_view: () => legacyInvoke<PageViewModel>("get_page_view"),
  set_viewport: (payload) =>
    legacyInvoke<PageViewModel>("set_viewport", {
      width: Number(payload?.width ?? 0),
      height: Number(payload?.height ?? 0),
    }),
  reload: () => legacyInvoke<PageViewModel>("reload"),
  back: () => legacyInvoke<PageViewModel>("back"),
  forward: () => legacyInvoke<PageViewModel>("forward"),
  get_navigation_state: () => legacyInvoke<NavigationState>("get_navigation_state"),
  new_tab: () => legacyInvoke<TabSummary>("new_tab"),
  duplicate_tab: (payload) => legacyInvoke<TabSummary>("duplicate_tab", { id: Number(payload?.id ?? 0) }),
  switch_tab: (payload) => legacyInvoke<PageViewModel>("switch_tab", { id: Number(payload?.id ?? 0) }),
  close_tab: (payload) => legacyInvoke<TabSummary[]>("close_tab", { id: Number(payload?.id ?? 0) }),
  move_tab: (payload) => legacyInvoke<TabSummary[]>("move_tab", {
    id: Number(payload?.id ?? 0),
    targetIndex: Number(payload?.target_index ?? 0),
  }),
  set_tab_pinned: (payload) => legacyInvoke<TabSummary[]>("set_tab_pinned", {
    id: Number(payload?.id ?? 0),
    pinned: Boolean(payload?.pinned),
  }),
  set_tab_muted: (payload) => legacyInvoke<TabSummary[]>("set_tab_muted", {
    id: Number(payload?.id ?? 0),
    muted: Boolean(payload?.muted),
  }),
  list_tabs: () => legacyInvoke<TabSummary[]>("list_tabs"),
  search: (payload) => legacyInvoke<SearchResult[]>("search", { query: `${payload?.query ?? ""}` }),
  omnibox_suggestions: (payload) => legacyInvoke<OmniboxSuggestionSet>("omnibox_suggestions", {
    query: `${payload?.query ?? ""}`,
    currentIndex: typeof payload?.current_index === "number" ? payload.current_index : null,
  }),
  update_scroll_positions: (payload) =>
    legacyInvoke<boolean>("update_scroll_positions", {
      positions: Array.isArray(payload?.positions) ? payload.positions : [],
    }),
  enqueue_download: (payload) => legacyInvoke<DownloadEntry>("enqueue_download", { url: `${payload?.url ?? ""}` }),
  list_downloads: () => legacyInvoke<DownloadEntry[]>("list_downloads"),
  get_download_progress: (payload) => legacyInvoke<DownloadEntry>("get_download_progress", { id: Number(payload?.id ?? 0) }),
  pause_download: (payload) => legacyInvoke<DownloadEntry>("pause_download", { id: Number(payload?.id ?? 0) }),
  resume_download: (payload) => legacyInvoke<DownloadEntry>("resume_download", { id: Number(payload?.id ?? 0) }),
  cancel_download: (payload) => legacyInvoke<DownloadEntry>("cancel_download", { id: Number(payload?.id ?? 0) }),
  open_download: (payload) => legacyInvoke<DownloadEntry>("open_download", { id: Number(payload?.id ?? 0) }),
  reveal_download: (payload) => legacyInvoke<DownloadEntry>("reveal_download", { id: Number(payload?.id ?? 0) }),
  get_download_policy_settings: () => legacyInvoke<DownloadPolicySettings>("get_download_policy_settings"),
  set_download_default_policy: (payload) => legacyInvoke<DownloadPolicySettings>("set_download_default_policy", {
    policy: payload?.policy,
  }),
  set_download_site_policy: (payload) => legacyInvoke<DownloadPolicySettings>("set_download_site_policy", {
    origin: `${payload?.origin ?? ""}`,
    policy: payload?.policy,
  }),
  clear_download_site_policy: (payload) => legacyInvoke<DownloadPolicySettings>("clear_download_site_policy", {
    origin: `${payload?.origin ?? ""}`,
  }),
};

const nativeTransport: AppTransport = {
  kind: "adapter_native",
  request: invokeNativeIpc,
};

const legacyTransport: AppTransport = {
  kind: "adapter_tauri",
  async request<T>(request: Omit<IpcRequest, "version">): Promise<T> {
    const handler = legacyTransportHandlers[request.type];
    if (!handler) {
      throw new Error(`Legacy fallback does not support ${request.type}`);
    }
    return (await handler(request.payload)) as T;
  },
};

let activeTransport: AppTransport | null = null;
let releaseRolloutStatus: ReleaseRolloutStatus | null = null;

function readTransportOverride(): TransportKind | null {
  try {
    const stored = window.localStorage.getItem(TRANSPORT_OVERRIDE_KEY);
    return stored === "adapter_native" || stored === "adapter_tauri" ? stored : null;
  } catch {
    return null;
  }
}

function writeTransportOverride(kind: TransportKind) {
  try {
    // Ref: HTML Living Standard, Web Storage stores string key/value data per origin
    // across top-level browsing sessions. We persist only the selected transport so
    // rollback decisions survive reloads without mutating application content state.
    // https://html.spec.whatwg.org/multipage/webstorage.html#the-localstorage-attribute
    window.localStorage.setItem(TRANSPORT_OVERRIDE_KEY, kind);
  } catch {
    // Ignore environments where storage is unavailable.
  }
}

function legacyTransportSupports(command: string): boolean {
  return command in legacyTransportHandlers;
}

function shouldFallbackToLegacy(errorValue: unknown): boolean {
  const message = errorValue instanceof Error ? errorValue.message : `${errorValue}`;
  return (
    message.includes("dispatch_ipc")
    || message.includes("Unsupported IPC response version")
    || message.includes("unknown command")
    || message.includes("invalid args")
  );
}

function activateTransport(kind: TransportKind, reason?: string) {
  activeTransport = kind === "adapter_native" ? nativeTransport : legacyTransport;
  writeTransportOverride(kind);
  if (reason) {
    console.warn(`[CosmoBrowse] transport switched to ${kind}: ${reason}`);
  }
}

async function initializeTransport(): Promise<AppTransport> {
  const override = readTransportOverride();
  if (override) {
    activeTransport = override === "adapter_native" ? nativeTransport : legacyTransport;
    return activeTransport;
  }

  try {
    const rolloutStatus = await invoke<ReleaseRolloutStatus>("release_rollout_status");
    releaseRolloutStatus = rolloutStatus;
    activeTransport = rolloutStatus.native_default_enabled ? nativeTransport : legacyTransport;
  } catch (errorValue) {
    console.warn("[CosmoBrowse] failed to load release rollout status; defaulting to adapter_native", errorValue);
    activeTransport = nativeTransport;
  }
  return activeTransport;
}

async function requestAppCommand<T>(request: Omit<IpcRequest, "version">): Promise<T> {
  const transport = activeTransport ?? (await initializeTransport());
  try {
    return await transport.request<T>(request);
  } catch (errorValue) {
    if (
      transport.kind === "adapter_native"
      && releaseRolloutStatus?.adapter_tauri_fallback_enabled !== false
      && legacyTransportSupports(request.type)
      && shouldFallbackToLegacy(errorValue)
    ) {
      activateTransport("adapter_tauri", errorValue instanceof Error ? errorValue.message : `${errorValue}`);
      return legacyTransport.request<T>(request);
    }
    throw errorValue;
  }
}

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
let networkLogEl: HTMLElement | null;
let consoleLogEl: HTMLElement | null;
let domSnapshotEl: HTMLElement | null;
let crashReportEl: HTMLElement | null;
let securityInterstitialEl: HTMLElement | null;
let downloadsListEl: HTMLUListElement | null;
let downloadSummaryEl: HTMLElement | null;
let downloadCurrentButtonEl: HTMLButtonElement | null;
let refreshDownloadsButtonEl: HTMLButtonElement | null;
let downloadDefaultDirInputEl: HTMLInputElement | null;
let downloadDefaultConfirmEl: HTMLInputElement | null;
let downloadDefaultApplyButtonEl: HTMLButtonElement | null;
let downloadSiteOriginInputEl: HTMLInputElement | null;
let downloadSiteDirInputEl: HTMLInputElement | null;
let downloadSiteConfirmEl: HTMLInputElement | null;
let downloadSiteApplyButtonEl: HTMLButtonElement | null;
let downloadSiteClearButtonEl: HTMLButtonElement | null;
let downloadPolicyStatusEl: HTMLElement | null;
let resizeObserver: ResizeObserver | null = null;
let resizeTimer: number | null = null;
let scrollSyncTimer: number | null = null;
let pageEpoch = 0;
let latestOmniboxSuggestions: OmniboxSuggestionSet = { query: "", active_index: null, suggestions: [] };
let draggedTabId: number | null = null;
let downloadPollTimer: number | null = null;
let latestDownloadPolicySettings: DownloadPolicySettings | null = null;

type RenderBackend = {
  kind: "native_scene";
  renderLeafFrame: (frame: FrameViewModel, shell: HTMLElement) => void;
};

const nativeSceneBackend: RenderBackend = {
  kind: "native_scene",
  renderLeafFrame(frame, shell) {
    const scene = document.createElement("div");
    scene.className = "frame-scene";
    // Ref: CSS 2.2 visual formatting model; absolutely positioned descendants are
    // laid out using inset offsets in their containing block.
    // https://www.w3.org/TR/CSS22/visuren.html#absolute-positioning
    scene.style.position = "relative";
    scene.style.width = `${Math.max(frame.content_size.width, frame.rect.width, 1)}px`;
    scene.style.height = `${Math.max(frame.content_size.height, frame.rect.height, 1)}px`;
    scene.style.overflow = "hidden";

    // Spec note: item order represents paint order from CSS2 visual formatting
    // model after CSS Display layout resolution.
    for (const command of frame.paint_commands) {
      scene.appendChild(renderPaintCommand(frame, command));
    }

    shell.appendChild(scene);
  },
};

function renderPaintCommand(frame: FrameViewModel, command: PaintCommand): HTMLElement {
  if (command.kind === "draw_rect") {
    const rect = document.createElement("div");
    rect.className = "scene-rect";
    rect.style.position = "absolute";
    rect.style.left = `${command.x}px`;
    rect.style.top = `${command.y}px`;
    rect.style.width = `${Math.max(command.width, 1)}px`;
    rect.style.height = `${Math.max(command.height, 1)}px`;
    rect.style.backgroundColor = command.background_color;
    rect.style.opacity = `${Math.min(Math.max(command.opacity, 0), 1)}`;
    return rect;
  }

  if (command.kind === "draw_text") {
    const textEl = document.createElement(command.href ? "button" : "span");
    textEl.className = command.href ? "scene-text scene-link" : "scene-text";
    textEl.textContent = command.text;
    textEl.style.position = "absolute";
    textEl.style.left = `${command.x}px`;
    textEl.style.top = `${command.y}px`;
    textEl.style.color = command.color;
    textEl.style.fontSize = `${Math.max(command.font_px, 1)}px`;
    textEl.style.fontFamily = command.font_family;
    textEl.style.opacity = `${Math.min(Math.max(command.opacity, 0), 1)}`;
    textEl.style.textDecoration = command.underline ? "underline" : "none";
    if (command.href) {
      // Ref: HTML Standard browsing context target keyword handling (`_blank`,
      // `_self`, `_parent`, `_top`) must use ASCII case-insensitive comparison.
      // https://html.spec.whatwg.org/multipage/browsing-the-web.html#valid-browsing-context-name-or-keyword
      textEl.addEventListener("click", () => {
        void handleEmbeddedNavigation({
          type: "cosmobrowse:navigate",
          frameId: frame.id,
          href: command.href ?? "",
          target: command.target ?? "",
        });
      });
    }
    return textEl;
  }

  const imgEl = document.createElement("img");
  imgEl.className = "scene-image";
  imgEl.alt = command.alt;
  imgEl.src = command.src;
  imgEl.style.position = "absolute";
  imgEl.style.left = `${command.x}px`;
  imgEl.style.top = `${command.y}px`;
  imgEl.style.width = `${Math.max(command.width, 1)}px`;
  imgEl.style.height = `${Math.max(command.height, 1)}px`;
  imgEl.style.opacity = `${Math.min(Math.max(command.opacity, 0), 1)}`;
  if (command.href) {
    const wrapper = document.createElement("button");
    wrapper.className = "scene-link-image";
    wrapper.style.position = "absolute";
    wrapper.style.left = `${command.x}px`;
    wrapper.style.top = `${command.y}px`;
    wrapper.style.width = `${Math.max(command.width, 1)}px`;
    wrapper.style.height = `${Math.max(command.height, 1)}px`;
    wrapper.style.padding = "0";
    wrapper.style.border = "none";
    wrapper.style.background = "transparent";
    wrapper.appendChild(imgEl);
    wrapper.addEventListener("click", () => {
      void handleEmbeddedNavigation({
        type: "cosmobrowse:navigate",
        frameId: frame.id,
        href: command.href ?? "",
        target: command.target ?? "",
      });
    });
    return wrapper;
  }
  return imgEl;
}

function resolveRenderBackend(frame: FrameViewModel): RenderBackend {
  // Backend replacement point: renderer selection is centralized here.
  // Spec note: regardless of incoming hint, rendering consumes scene items that
  // already encode HTML LS parsing + DOM Standard updates + CSS layout/paint order.
  switch (frame.render_backend) {
    case "web_view":
      return nativeSceneBackend;
    case "native_scene":
      return nativeSceneBackend;
  }
}

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

function parseAppError(errorValue: unknown): AppError {
  const error = errorValue as Partial<AppError>;
  return {
    code: error.code ?? "unknown_error",
    message: error.message ?? "Unknown error",
    retryable: Boolean(error.retryable),
  };
}

function formatError(errorValue: unknown): string {
  const error = parseAppError(errorValue);
  const descriptions: Record<string, string> = {
    network_timeout: "The request timed out. Check your network connection and try again.",
    network_redirect_loop: "Navigation was stopped because the redirect chain exceeded the limit.",
    network_content_decoding: "The response body could not be decoded successfully.",
    cors_blocked: "The response was blocked by the CORS policy.",
    cors_preflight_failed: "The CORS preflight validation failed.",
    tls_error: "The certificate or TLS configuration could not be validated.",
    tls_certificate_expired: "The certificate has expired.",
    tls_certificate_self_signed: "The connection uses a self-signed certificate and cannot be trusted automatically.",
    download_required: "The response is marked for download and will not be rendered inline.",
    download_save_failed: "The file could not be written to the selected download location.",
    download_permission_denied: "CosmoBrowse does not have permission to write to the download location.",
    download_resume_unsupported: "The server does not support HTTP range resume, so the download restarts from the beginning.",
  };
  const base = descriptions[error.code] ? `${descriptions[error.code]} (${error.code})` : error.code;
  return `${base}: ${error.message}`;
}

function formatBytes(value: number | null): string {
  if (value == null || !Number.isFinite(value)) return "?";
  const units = ["B", "KB", "MB", "GB"];
  let amount = value;
  let unitIndex = 0;
  while (amount >= 1024 && unitIndex < units.length - 1) {
    amount /= 1024;
    unitIndex += 1;
  }
  return `${amount.toFixed(unitIndex === 0 ? 0 : 1)} ${units[unitIndex]}`;
}

function downloadProgressRatio(entry: DownloadEntry): number | null {
  if (!entry.total_bytes || entry.total_bytes <= 0) return null;
  return Math.max(0, Math.min(entry.downloaded_bytes / entry.total_bytes, 1));
}

function parentDirectory(path: string): string {
  const separatorIndex = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return separatorIndex >= 0 ? path.slice(0, separatorIndex) : path;
}

function pathToFileUrl(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  return normalized.startsWith("/") ? `file://${encodeURI(normalized)}` : `file:///${encodeURI(normalized)}`;
}

function hideSecurityInterstitial() {
  if (!securityInterstitialEl) return;
  securityInterstitialEl.classList.add("hidden");
  securityInterstitialEl.innerHTML = "";
}

async function registerTlsExceptionAndRetry(url: string) {
  // Spec: certificate exception registration must be explicit user action.
  // Ref: browser interstitial UX guidance for dangerous TLS connections.
  try {
    await invokeNativeIpc<boolean>({ type: "register_tls_exception", payload: { url } });
    setStatus("A temporary certificate exception was registered. Retrying the connection...");
    await openUrl(url);
  } catch (errorValue) {
    handleCommandError(errorValue, url);
  }
}

function maybeRenderSecurityInterstitial(context: ErrorContext) {
  if (!securityInterstitialEl) return;
  const { appError, requestedUrl } = context;
  const tlsCodes = new Set(["tls_error", "tls_certificate_expired", "tls_certificate_self_signed"]);
  if (!tlsCodes.has(appError.code) || !requestedUrl) {
    hideSecurityInterstitial();
    return;
  }

  securityInterstitialEl.classList.remove("hidden");
  securityInterstitialEl.innerHTML = "";

  const heading = document.createElement("h3");
  heading.textContent = "Potentially unsafe connection detected";
  const detail = document.createElement("p");
  detail.textContent = `${formatError(appError)}
URL: ${requestedUrl}`;
  const guidance = document.createElement("p");
  guidance.textContent = "This connection may expose the page to interception or tampering. Proceed only if you understand the risk.";

  const actions = document.createElement("div");
  actions.className = "interstitial-actions";
  const backButton = document.createElement("button");
  backButton.type = "button";
  backButton.textContent = "Go back";
  backButton.addEventListener("click", () => hideSecurityInterstitial());

  const proceedButton = document.createElement("button");
  proceedButton.type = "button";
  proceedButton.className = "danger-button";
  proceedButton.textContent = "Proceed anyway (15 min)";
  proceedButton.addEventListener("click", () => void registerTlsExceptionAndRetry(requestedUrl));

  actions.append(backButton, proceedButton);
  securityInterstitialEl.append(heading, detail, guidance, actions);
}

function handleCommandError(errorValue: unknown, requestedUrl?: string) {
  const appError = parseAppError(errorValue);
  const downloadHint = appError.code === "download_required" && requestedUrl
    ? ` Use the download shelf to explicitly save ${requestedUrl}.`
    : "";
  setStatus(`${formatError(appError)}${downloadHint}`, "error");
  maybeRenderSecurityInterstitial({ appError, requestedUrl });
}

function clearSearchResults() {
  latestOmniboxSuggestions = { query: "", active_index: null, suggestions: [] };
  if (!searchResultsEl) return;
  searchResultsEl.innerHTML = "";
  searchResultsEl.classList.add("hidden");
}

function normalizeActiveSuggestionIndex(resultSet: OmniboxSuggestionSet, requestedIndex?: number | null): OmniboxSuggestionSet {
  if (resultSet.suggestions.length === 0) {
    return { ...resultSet, active_index: null };
  }
  const activeIndex = requestedIndex ?? resultSet.active_index ?? 0;
  return {
    ...resultSet,
    active_index: Math.max(0, Math.min(activeIndex, resultSet.suggestions.length - 1)),
  };
}

function suggestionTargetUrl(suggestion: OmniboxSuggestion): string {
  return suggestion.url ?? suggestion.value;
}

async function activateSuggestion(suggestion: OmniboxSuggestion) {
  clearSearchResults();
  const targetUrl = suggestionTargetUrl(suggestion);
  if (urlInputEl) urlInputEl.value = targetUrl;
  await openUrl(targetUrl);
}

function renderSearchResults(resultSet: OmniboxSuggestionSet) {
  latestOmniboxSuggestions = normalizeActiveSuggestionIndex(resultSet);
  if (!searchResultsEl) return;
  const host = searchResultsEl;
  host.innerHTML = "";
  host.classList.toggle("hidden", latestOmniboxSuggestions.suggestions.length === 0);
  latestOmniboxSuggestions.suggestions.forEach((suggestion, index) => {
    const item = document.createElement("li");
    item.className = index === latestOmniboxSuggestions.active_index ? "selected" : "";
    item.setAttribute("role", "option");
    item.setAttribute("aria-selected", `${index === latestOmniboxSuggestions.active_index}`);

    const button = document.createElement("button");
    button.type = "button";
    button.className = "linklike suggestion-button";
    button.addEventListener("mouseenter", () => {
      latestOmniboxSuggestions = normalizeActiveSuggestionIndex(latestOmniboxSuggestions, index);
      renderSearchResults(latestOmniboxSuggestions);
    });
    button.addEventListener("click", () => void activateSuggestion(suggestion));

    const kind = document.createElement("span");
    kind.className = "suggestion-kind";
    kind.textContent = suggestion.kind;

    const title = document.createElement("span");
    title.className = "suggestion-title";
    title.textContent = suggestion.title;

    const meta = document.createElement("small");
    meta.className = "suggestion-meta";
    meta.textContent = `${suggestionTargetUrl(suggestion)} — ${suggestion.detail}`;

    button.append(kind, title, meta);
    item.appendChild(button);
    host.appendChild(item);
  });
}

async function updateOmniboxSuggestions(currentIndex?: number | null) {
  if (!urlInputEl) return;
  const query = urlInputEl.value.trim();
  try {
    const suggestions = await requestAppCommand<OmniboxSuggestionSet>({
      type: "omnibox_suggestions",
      payload: {
        query,
        current_index: currentIndex ?? latestOmniboxSuggestions.active_index,
      },
    });
    renderSearchResults(normalizeActiveSuggestionIndex(suggestions, currentIndex));
  } catch (errorValue) {
    handleCommandError(errorValue);
  }
}

async function moveOmniboxSelection(delta: number) {
  if (latestOmniboxSuggestions.suggestions.length === 0) {
    await updateOmniboxSuggestions(delta > 0 ? 0 : null);
    return;
  }
  const currentIndex = latestOmniboxSuggestions.active_index ?? 0;
  const nextIndex = (currentIndex + delta + latestOmniboxSuggestions.suggestions.length)
    % latestOmniboxSuggestions.suggestions.length;
  renderSearchResults(normalizeActiveSuggestionIndex(latestOmniboxSuggestions, nextIndex));
}

async function commitOmniboxSelection() {
  const selectedIndex = latestOmniboxSuggestions.active_index;
  if (selectedIndex != null) {
    const selected = latestOmniboxSuggestions.suggestions[selectedIndex];
    if (selected) {
      await activateSuggestion(selected);
      return;
    }
  }
  await openCurrentInput();
}

function renderTabs(tabs: TabSummary[]) {
  if (!tabsListEl) return;
  tabsListEl.innerHTML = "";
  for (const [index, tab] of tabs.entries()) {
    const item = document.createElement("li");
    item.className = `tab-item${tab.is_active ? " active" : ""}${tab.is_pinned ? " pinned" : ""}`;
    item.draggable = true;
    item.dataset.tabId = `${tab.id}`;
    item.dataset.tabIndex = `${index}`;

    // Ref: HTML Standard drag-and-drop dispatches dragstart/dragover/drop in the
    // document event loop; we only mutate ordering on drop so the visible tab strip
    // remains consistent with the final user-selected insertion point.
    // https://html.spec.whatwg.org/multipage/dnd.html#drag-and-drop-processing-model
    item.addEventListener("dragstart", (event) => {
      draggedTabId = tab.id;
      item.classList.add("dragging");
      event.dataTransfer?.setData("text/plain", `${tab.id}`);
      if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
    });
    item.addEventListener("dragend", () => {
      draggedTabId = null;
      tabsListEl?.querySelectorAll(".tab-item").forEach((node) => node.classList.remove("dragging", "drop-target"));
    });
    item.addEventListener("dragover", (event) => {
      event.preventDefault();
      item.classList.add("drop-target");
      if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
    });
    item.addEventListener("dragleave", () => item.classList.remove("drop-target"));
    item.addEventListener("drop", (event) => {
      event.preventDefault();
      item.classList.remove("drop-target");
      const sourceId = draggedTabId ?? Number(event.dataTransfer?.getData("text/plain") ?? 0);
      if (!Number.isFinite(sourceId) || sourceId === tab.id) return;
      void moveTab(sourceId, index);
    });

    const dragHandle = document.createElement("button");
    dragHandle.type = "button";
    dragHandle.className = "tab-drag-handle";
    dragHandle.textContent = "⋮⋮";
    dragHandle.title = "Drag to reorder";
    dragHandle.setAttribute("aria-label", `Reorder ${tab.title || tab.url || "tab"}`);

    const switchButton = document.createElement("button");
    switchButton.type = "button";
    switchButton.className = "tab-switch";
    switchButton.addEventListener("click", () => void switchTab(tab.id));

    const stateIcons = document.createElement("span");
    stateIcons.className = "tab-state-icons";
    stateIcons.textContent = `${tab.is_pinned ? "📌" : ""}${tab.is_muted ? "🔇" : ""}` || "";

    const title = document.createElement("span");
    title.className = "tab-title";
    title.textContent = tab.title || tab.url || "New Tab";
    switchButton.append(stateIcons, title);

    const pinButton = document.createElement("button");
    pinButton.type = "button";
    pinButton.className = "tab-action";
    pinButton.textContent = tab.is_pinned ? "📍" : "📌";
    pinButton.title = tab.is_pinned ? "Unpin tab" : "Pin tab";
    pinButton.addEventListener("click", (event) => {
      event.stopPropagation();
      void setTabPinned(tab.id, !tab.is_pinned);
    });

    const muteButton = document.createElement("button");
    muteButton.type = "button";
    muteButton.className = "tab-action";
    muteButton.textContent = tab.is_muted ? "🔈" : "🔇";
    muteButton.title = tab.is_muted ? "Unmute tab" : "Mute tab";
    muteButton.addEventListener("click", (event) => {
      event.stopPropagation();
      void setTabMuted(tab.id, !tab.is_muted);
    });

    const duplicateButton = document.createElement("button");
    duplicateButton.type = "button";
    duplicateButton.className = "tab-action";
    duplicateButton.textContent = "⧉";
    duplicateButton.title = "Duplicate tab";
    duplicateButton.addEventListener("click", (event) => {
      event.stopPropagation();
      void duplicateTab(tab.id);
    });

    const closeButton = document.createElement("button");
    closeButton.type = "button";
    closeButton.className = "tab-close";
    closeButton.textContent = "x";
    closeButton.addEventListener("click", (event) => {
      event.stopPropagation();
      void closeTab(tab.id);
    });

    item.append(dragHandle, switchButton, pinButton, muteButton, duplicateButton, closeButton);
    tabsListEl.appendChild(item);
  }
}

function renderRenderTreeNode(node: RenderNode, container: HTMLElement, depth = 0) {
  const line = document.createElement("div");
  line.className = `render-tree-node kind-${node.kind}`;
  line.style.paddingLeft = `${depth * 12}px`;
  const label = node.text?.trim() ? `${node.node_name} "${node.text.trim().slice(0, 24)}"` : node.node_name;
  line.textContent = `${node.kind.toUpperCase()} ${label} [${node.box_info.x},${node.box_info.y} ${node.box_info.width}x${node.box_info.height}]`;
  line.title = `display=${node.style.display}; position=${node.style.position}; color=${node.style.color}; bg=${node.style.background_color}`;
  container.appendChild(line);
  for (const child of node.children) {
    renderRenderTreeNode(child, container, depth + 1);
  }
}

function renderRenderTree(frame: FrameViewModel): HTMLElement | null {
  const root = frame.render_tree?.root;
  if (!root) return null;
  const panel = document.createElement("aside");
  panel.className = "render-tree-panel";
  const heading = document.createElement("p");
  heading.className = "render-tree-title";
  heading.textContent = `RenderTree: ${frame.id}`;
  panel.appendChild(heading);
  const body = document.createElement("div");
  body.className = "render-tree-body";
  renderRenderTreeNode(root, body);
  panel.appendChild(body);
  return panel;
}

function renderFrame(frame: FrameViewModel, host: HTMLElement) {
  const shell = document.createElement("section");
  shell.className = frame.child_frames.length > 0 ? "frame-shell frameset-shell" : "frame-shell leaf-shell";
  // Ref: CSS 2.2 visual formatting model; absolutely positioned boxes use inset offsets.
  // https://www.w3.org/TR/CSS22/visuren.html#absolute-positioning
  shell.style.left = `${frame.rect.x}px`;
  shell.style.top = `${frame.rect.y}px`;
  shell.style.width = `${Math.max(frame.rect.width, 1)}px`;
  shell.style.height = `${Math.max(frame.rect.height, 1)}px`;
  shell.dataset.frameId = frame.id;
  shell.style.overflow = frame.child_frames.length > 0 ? "hidden" : "auto";
  shell.addEventListener("scroll", () => scheduleScrollSync(), { passive: true });

  if (frame.child_frames.length > 0) {
    const childrenRoot = document.createElement("div");
    childrenRoot.className = "frame-children";
    for (const child of frame.child_frames) {
      renderFrame(child, childrenRoot);
    }
    shell.appendChild(childrenRoot);
  } else {
    const backend = resolveRenderBackend(frame);
    backend.renderLeafFrame(frame, shell);
    shell.dataset.renderBackend = backend.kind;
    const renderTreePanel = renderRenderTree(frame);
    if (renderTreePanel) shell.appendChild(renderTreePanel);
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

function applyRestoredScrollPositions(view: PageViewModel) {
  // Ref: HTML Standard history traversal restores a session history entry's
  // persisted user state after activating the entry. We map that persisted user
  // state to viewport/frame scroll offsets captured in the session snapshot.
  // https://html.spec.whatwg.org/multipage/nav-history-apis.html#persisted-user-state-restoration
  window.requestAnimationFrame(() => {
    if (viewportEl) {
      viewportEl.scrollTo({
        left: Math.max(view.root_frame.scroll_position.x, 0),
        top: Math.max(view.root_frame.scroll_position.y, 0),
      });
    }

    const frameScrolls = new Map<string, ScrollPosition>();
    collectFrameScrollPositions(view.root_frame, frameScrolls);
    sceneRootEl?.querySelectorAll<HTMLElement>(".frame-shell[data-frame-id]").forEach((shell) => {
      const frameId = shell.dataset.frameId;
      if (!frameId) return;
      const position = frameScrolls.get(frameId);
      if (!position) return;
      shell.scrollTo({ left: Math.max(position.x, 0), top: Math.max(position.y, 0) });
    });
  });
}

function collectFrameScrollPositions(frame: FrameViewModel, out: Map<string, ScrollPosition>) {
  out.set(frame.id, frame.scroll_position);
  for (const child of frame.child_frames) {
    collectFrameScrollPositions(child, out);
  }
}


function renderDiagnostics(view: PageViewModel) {
  if (networkLogEl) networkLogEl.textContent = view.network_log.length > 0 ? view.network_log.join("\n") : "(no network diagnostics)";
  if (consoleLogEl) consoleLogEl.textContent = view.console_log.length > 0 ? view.console_log.join("\n") : "(no console diagnostics)";
  if (domSnapshotEl) {
    domSnapshotEl.textContent = view.dom_snapshot.length > 0
      ? view.dom_snapshot
          .map((entry) => `# ${entry.frame_id} (${entry.document_url})\n${entry.html}`)
          .join("\n\n")
      : "(no DOM snapshots)";
  }
}

async function refreshCrashReport() {
  try {
    const report = await invokeNativeIpc<{
      path: string;
      reason: string;
      crashed_at_ms: number;
      build_id: string;
      commit_hash: string;
      transport: string;
      active_url: string;
      last_command: string;
      reproduction: string[];
    } | null>({ type: "get_latest_crash_report" });
    if (!crashReportEl) return;
    crashReportEl.textContent = report
      ? `Crash report: ${report.reason} @ ${new Date(report.crashed_at_ms).toISOString()} (${report.transport || "unknown transport"}, ${report.active_url || "no url"}, ${report.last_command || "no command"})`
      : "Crash report: none";
  } catch {
    if (crashReportEl) crashReportEl.textContent = "Crash report: unavailable";
  }
}

function renderDownloads(downloads: DownloadEntry[]) {
  if (downloadsListEl) downloadsListEl.innerHTML = "";
  if (downloadSummaryEl) {
    const activeCount = downloads.filter((entry) => entry.state === "queued" || entry.state === "downloading" || entry.state === "paused").length;
    downloadSummaryEl.textContent = downloads.length === 0
      ? "No downloads yet."
      : `${downloads.length} item(s), ${activeCount} active.`;
  }
  if (!downloadsListEl) return;

  downloads
    .slice()
    .sort((left, right) => right.created_at_ms - left.created_at_ms)
    .forEach((entry) => {
      const item = document.createElement("li");
      item.className = `download-item state-${entry.state}`;

      const header = document.createElement("div");
      header.className = "download-item-header";
      const title = document.createElement("strong");
      title.textContent = entry.file_name;
      const badge = document.createElement("span");
      badge.className = "download-state-badge";
      badge.textContent = entry.state;
      header.append(title, badge);

      const meta = document.createElement("p");
      meta.className = "download-meta";
      const ratio = downloadProgressRatio(entry);
      meta.textContent = ratio == null
        ? `${formatBytes(entry.downloaded_bytes)} downloaded • save to ${entry.save_path}`
        : `${Math.round(ratio * 100)}% • ${formatBytes(entry.downloaded_bytes)} / ${formatBytes(entry.total_bytes)} • save to ${entry.save_path}`;

      const progress = document.createElement("div");
      progress.className = "download-progress";
      const progressBar = document.createElement("span");
      progressBar.className = "download-progress-bar";
      progressBar.style.width = `${Math.round((ratio ?? 0) * 100)}%`;
      progress.appendChild(progressBar);

      const message = document.createElement("p");
      message.className = "download-message";
      const resumeLabel = entry.supports_resume === false ? "Resume unsupported by server; restart fallback used." : entry.supports_resume === true ? "Resume supported." : "Resume capability pending.";
      message.textContent = [entry.status_message, resumeLabel, entry.last_error ? `${entry.last_error.code}: ${entry.last_error.message}` : null]
        .filter((value): value is string => Boolean(value && value.trim().length > 0))
        .join(" ");

      const actions = document.createElement("div");
      actions.className = "download-actions";

      const pauseButton = document.createElement("button");
      pauseButton.type = "button";
      pauseButton.textContent = "Pause";
      pauseButton.disabled = !(entry.state === "queued" || entry.state === "downloading");
      pauseButton.addEventListener("click", () => void controlDownload("pause_download", entry.id));

      const resumeButton = document.createElement("button");
      resumeButton.type = "button";
      resumeButton.textContent = entry.state === "failed" ? "Retry" : "Resume";
      resumeButton.disabled = !(entry.state === "paused" || entry.state === "failed");
      resumeButton.addEventListener("click", () => void controlDownload("resume_download", entry.id));

      const cancelButton = document.createElement("button");
      cancelButton.type = "button";
      cancelButton.textContent = entry.state === "completed" ? "Done" : "Cancel";
      cancelButton.disabled = !(entry.state === "queued" || entry.state === "downloading" || entry.state === "paused");
      cancelButton.addEventListener("click", () => void controlDownload("cancel_download", entry.id));

      const openButton = document.createElement("button");
      openButton.type = "button";
      openButton.textContent = "Open";
      openButton.disabled = entry.state !== "completed";
      openButton.addEventListener("click", () => void triggerDownloadOpen(entry.id));

      const revealButton = document.createElement("button");
      revealButton.type = "button";
      revealButton.textContent = "Reveal";
      revealButton.disabled = entry.state !== "completed";
      revealButton.addEventListener("click", () => void triggerDownloadReveal(entry.id));

      actions.append(pauseButton, resumeButton, cancelButton, openButton, revealButton);
      item.append(header, meta, progress, message, actions);
      downloadsListEl?.appendChild(item);
    });
}

async function refreshDownloads(silent = false) {
  try {
    const downloads = await requestAppCommand<DownloadEntry[]>({ type: "list_downloads" });
    renderDownloads(downloads);
  } catch (errorValue) {
    if (!silent) handleCommandError(errorValue);
  }
}

function currentDownloadSiteOriginCandidate(): string | null {
  const raw = (downloadSiteOriginInputEl?.value ?? urlInputEl?.value ?? "").trim();
  if (!raw) return null;
  try {
    const parsed = new URL(raw);
    if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return null;
    return parsed.origin;
  } catch {
    return null;
  }
}

function applyDownloadPolicyInputs(settings: DownloadPolicySettings) {
  if (downloadDefaultDirInputEl) downloadDefaultDirInputEl.value = settings.default_policy.directory;
  if (downloadDefaultConfirmEl) downloadDefaultConfirmEl.checked = settings.default_policy.requires_user_confirmation;

  const origin = currentDownloadSiteOriginCandidate();
  if (origin && downloadSiteOriginInputEl) downloadSiteOriginInputEl.value = origin;
  const sitePolicy = origin
    ? settings.site_policies.find((entry) => entry.origin === origin)
    : null;
  if (downloadSiteDirInputEl) {
    downloadSiteDirInputEl.value = sitePolicy?.policy.directory ?? settings.default_policy.directory;
  }
  if (downloadSiteConfirmEl) {
    downloadSiteConfirmEl.checked = sitePolicy?.policy.requires_user_confirmation ?? settings.default_policy.requires_user_confirmation;
  }
  if (downloadPolicyStatusEl) {
    downloadPolicyStatusEl.textContent = origin
      ? sitePolicy
        ? `Site policy active for ${origin}.`
        : `No site override for ${origin}; default policy applies.`
      : "Enter an http(s) origin to edit site-specific policy.";
  }
}

async function refreshDownloadPolicySettings(silent = false) {
  try {
    const settings = await requestAppCommand<DownloadPolicySettings>({ type: "get_download_policy_settings" });
    latestDownloadPolicySettings = settings;
    applyDownloadPolicyInputs(settings);
  } catch (errorValue) {
    if (!silent) handleCommandError(errorValue);
  }
}

async function applyDefaultDownloadPolicy() {
  const directory = downloadDefaultDirInputEl?.value.trim() ?? "";
  const requiresUserConfirmation = Boolean(downloadDefaultConfirmEl?.checked);
  try {
    const settings = await requestAppCommand<DownloadPolicySettings>({
      type: "set_download_default_policy",
      payload: {
        policy: {
          directory,
          conflict_policy: "uniquify",
          requires_user_confirmation: requiresUserConfirmation,
        },
      },
    });
    latestDownloadPolicySettings = settings;
    applyDownloadPolicyInputs(settings);
    setStatus("Updated default download policy.");
  } catch (errorValue) {
    handleCommandError(errorValue);
  }
}

async function applySiteDownloadPolicy() {
  const origin = currentDownloadSiteOriginCandidate();
  if (!origin) {
    setStatus("Site policy requires a valid http(s) origin.", "error");
    return;
  }
  const directory = downloadSiteDirInputEl?.value.trim() ?? "";
  const requiresUserConfirmation = Boolean(downloadSiteConfirmEl?.checked);
  try {
    const settings = await requestAppCommand<DownloadPolicySettings>({
      type: "set_download_site_policy",
      payload: {
        origin,
        policy: {
          directory,
          conflict_policy: "uniquify",
          requires_user_confirmation: requiresUserConfirmation,
        },
      },
    });
    latestDownloadPolicySettings = settings;
    applyDownloadPolicyInputs(settings);
    setStatus(`Updated site policy: ${origin}`);
  } catch (errorValue) {
    handleCommandError(errorValue);
  }
}

async function clearSiteDownloadPolicy() {
  const origin = currentDownloadSiteOriginCandidate();
  if (!origin) {
    setStatus("Site policy clear requires a valid http(s) origin.", "error");
    return;
  }
  try {
    const settings = await requestAppCommand<DownloadPolicySettings>({
      type: "clear_download_site_policy",
      payload: { origin },
    });
    latestDownloadPolicySettings = settings;
    applyDownloadPolicyInputs(settings);
    setStatus(`Cleared site policy: ${origin}`);
  } catch (errorValue) {
    handleCommandError(errorValue);
  }
}

async function enqueueCurrentUrlDownload() {
  const candidate = urlInputEl?.value.trim() ?? "";
  if (!candidate) {
    setStatus("Enter or navigate to a URL before starting a download.", "error");
    return;
  }
  try {
    const entry = await requestAppCommand<DownloadEntry>({
      type: "enqueue_download",
      payload: { url: candidate },
    });
    setStatus(`Download queued: ${entry.file_name}`);
    await refreshDownloads(true);
  } catch (errorValue) {
    handleCommandError(errorValue, candidate);
  }
}

async function controlDownload(command: "pause_download" | "resume_download" | "cancel_download", id: number) {
  try {
    await requestAppCommand<DownloadEntry>({ type: command, payload: { id } });
    await refreshDownloads(true);
  } catch (errorValue) {
    handleCommandError(errorValue);
  }
}

async function triggerDownloadOpen(id: number) {
  try {
    const entry = await requestAppCommand<DownloadEntry>({ type: "open_download", payload: { id } });
    await openExternalUrl(pathToFileUrl(entry.save_path));
    setStatus(`Opened download: ${entry.file_name}`);
  } catch (errorValue) {
    handleCommandError(errorValue);
  }
}

async function triggerDownloadReveal(id: number) {
  try {
    const entry = await requestAppCommand<DownloadEntry>({ type: "reveal_download", payload: { id } });
    await openExternalUrl(pathToFileUrl(parentDirectory(entry.save_path)));
    setStatus(`Opened download folder for: ${entry.file_name}`);
  } catch (errorValue) {
    handleCommandError(errorValue);
  }
}

function renderPageView(view: PageViewModel) {
  hideSecurityInterstitial();
  if (currentUrlEl) currentUrlEl.textContent = view.current_url ? `URL: ${view.current_url}` : "URL: (none)";
  if (pageTitleEl) pageTitleEl.textContent = view.title || "New Tab";
  if (urlInputEl && view.current_url) urlInputEl.value = view.current_url;
  document.title = view.title ? `${view.title} - CosmoBrowse` : "CosmoBrowse";
  renderFrameTree(view);
  applyRestoredScrollPositions(view);
  renderDiagnostics(view);
  const diagnostics = view.diagnostics.filter((entry) => entry.trim().length > 0);
  setStatus(diagnostics.length > 0 ? diagnostics.join(" | ") : "Done.");
}

function collectCurrentScrollPositions(): FrameScrollPositionSnapshot[] {
  const positions: FrameScrollPositionSnapshot[] = [];
  if (viewportEl) {
    positions.push({
      frame_id: "root",
      position: {
        x: Math.round(viewportEl.scrollLeft),
        y: Math.round(viewportEl.scrollTop),
      },
    });
  }
  sceneRootEl?.querySelectorAll<HTMLElement>(".frame-shell[data-frame-id]").forEach((shell) => {
    const frameId = shell.dataset.frameId;
    if (!frameId) return;
    positions.push({
      frame_id: frameId,
      position: {
        x: Math.round(shell.scrollLeft),
        y: Math.round(shell.scrollTop),
      },
    });
  });
  return positions;
}

async function flushScrollPositions() {
  const positions = collectCurrentScrollPositions();
  if (positions.length === 0) return;
  try {
    await requestAppCommand<boolean>({
      type: "update_scroll_positions",
      payload: { positions },
    });
  } catch {
    // Ignore best-effort persistence failures during interactive scrolling.
  }
}

function scheduleScrollSync() {
  if (scrollSyncTimer) window.clearTimeout(scrollSyncTimer);
  scrollSyncTimer = window.setTimeout(() => {
    scrollSyncTimer = null;
    void flushScrollPositions();
  }, 120);
}

async function refreshTabs() {
  const tabs = await requestAppCommand<TabSummary[]>({ type: "list_tabs" });
  renderTabs(tabs);
}

async function refreshNavigationState() {
  try {
    const nav = await requestAppCommand<NavigationState>({ type: "get_navigation_state" });
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
      const view = await requestAppCommand<PageViewModel>({ type: "set_viewport", payload: { width, height } });
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
    const view = await requestAppCommand<PageViewModel>({ type: command });
    if (!isCurrentPageEpoch(epoch)) return;
    renderPageView(view);
    await refreshNavigationState();
    await refreshTabs();
  } catch (errorValue) {
    if (!isCurrentPageEpoch(epoch)) return;
    handleCommandError(errorValue);
  }
}

async function openUrl(url: string) {
  const epoch = beginPageEpoch();
  try {
    clearSearchResults();
    setStatus("Loading...");
    const view = await requestAppCommand<PageViewModel>({ type: "open_url", payload: { url } });
    if (!isCurrentPageEpoch(epoch)) return;
    renderPageView(view);
    await refreshNavigationState();
    await refreshTabs();
  } catch (errorValue) {
    if (!isCurrentPageEpoch(epoch)) return;
    handleCommandError(errorValue, url);
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
      handleCommandError(errorValue);
    }
    return;
  }

  const epoch = beginPageEpoch();
  try {
    setStatus("Navigating...");
    const view = await requestAppCommand<PageViewModel>({
      type: "activate_link",
      payload: {
        frame_id: message.frameId,
        href: message.href,
        target: normalizedTarget,
      },
    });
    if (!isCurrentPageEpoch(epoch)) return;
    renderPageView(view);
    await refreshNavigationState();
    await refreshTabs();
  } catch (errorValue) {
    if (!isCurrentPageEpoch(epoch)) return;
    handleCommandError(errorValue, message.href);
  }
}
async function openCurrentInput() {
  if (!urlInputEl) return;
  const value = urlInputEl.value.trim();
  if (!value) {
    setStatus("Please enter a URL or search query.", "error");
    return;
  }

  if (latestOmniboxSuggestions.suggestions.length > 0) {
    await commitOmniboxSelection();
    return;
  }

  if (isUrlLikeInput(value)) {
    await openUrl(value);
    return;
  }

  await updateOmniboxSuggestions(0);
  if (latestOmniboxSuggestions.suggestions.length > 0) {
    setStatus(`Found ${latestOmniboxSuggestions.suggestions.length} suggestions.`);
    return;
  }

  await openUrl(value);
}

async function switchTab(id: number) {
  const epoch = beginPageEpoch();
  try {
    clearSearchResults();
    const view = await requestAppCommand<PageViewModel>({ type: "switch_tab", payload: { id } });
    if (!isCurrentPageEpoch(epoch)) return;
    renderPageView(view);
    await refreshNavigationState();
    await refreshTabs();
  } catch (errorValue) {
    if (!isCurrentPageEpoch(epoch)) return;
    handleCommandError(errorValue);
  }
}

async function duplicateTab(id: number) {
  const epoch = beginPageEpoch();
  try {
    clearSearchResults();
    await requestAppCommand<TabSummary>({ type: "duplicate_tab", payload: { id } });
    if (!isCurrentPageEpoch(epoch)) return;
    const view = await requestAppCommand<PageViewModel>({ type: "get_page_view" });
    if (!isCurrentPageEpoch(epoch)) return;
    renderPageView(view);
    await refreshNavigationState();
    await refreshTabs();
  } catch (errorValue) {
    if (!isCurrentPageEpoch(epoch)) return;
    handleCommandError(errorValue);
  }
}

async function moveTab(id: number, targetIndex: number) {
  try {
    await requestAppCommand<TabSummary[]>({ type: "move_tab", payload: { id, target_index: targetIndex } });
    await refreshTabs();
  } catch (errorValue) {
    handleCommandError(errorValue);
  }
}

async function setTabPinned(id: number, pinned: boolean) {
  try {
    await requestAppCommand<TabSummary[]>({ type: "set_tab_pinned", payload: { id, pinned } });
    await refreshTabs();
  } catch (errorValue) {
    handleCommandError(errorValue);
  }
}

async function setTabMuted(id: number, muted: boolean) {
  try {
    await requestAppCommand<TabSummary[]>({ type: "set_tab_muted", payload: { id, muted } });
    await refreshTabs();
  } catch (errorValue) {
    handleCommandError(errorValue);
  }
}

async function closeTab(id: number) {
  const epoch = beginPageEpoch();
  try {
    clearSearchResults();
    await requestAppCommand<TabSummary[]>({ type: "close_tab", payload: { id } });
    if (!isCurrentPageEpoch(epoch)) return;
    const view = await requestAppCommand<PageViewModel>({ type: "get_page_view" });
    if (!isCurrentPageEpoch(epoch)) return;
    renderPageView(view);
    await refreshNavigationState();
    await refreshTabs();
  } catch (errorValue) {
    if (!isCurrentPageEpoch(epoch)) return;
    handleCommandError(errorValue);
  }
}

async function createTab() {
  const epoch = beginPageEpoch();
  try {
    clearSearchResults();
    await requestAppCommand<TabSummary>({ type: "new_tab" });
    if (!isCurrentPageEpoch(epoch)) return;
    const view = await requestAppCommand<PageViewModel>({ type: "get_page_view" });
    if (!isCurrentPageEpoch(epoch)) return;
    renderPageView(view);
    await refreshNavigationState();
    await refreshTabs();
  } catch (errorValue) {
    if (!isCurrentPageEpoch(epoch)) return;
    handleCommandError(errorValue);
  }
}

function focusAddressInput() {
  urlInputEl?.focus();
  urlInputEl?.select();
}

function shouldHandleGlobalShortcut(event: KeyboardEvent): boolean {
  const target = event.target;
  return !(target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement || (target instanceof HTMLElement && target.isContentEditable));
}

function installKeyboardHandlers() {
  urlInputEl?.addEventListener("input", () => void updateOmniboxSuggestions());
  urlInputEl?.addEventListener("focus", () => void updateOmniboxSuggestions());
  urlInputEl?.addEventListener("keydown", (event) => {
    if (event.key === "Enter") {
      event.preventDefault();
      void commitOmniboxSelection();
      return;
    }
    if (event.key === "ArrowDown") {
      event.preventDefault();
      void moveOmniboxSelection(1);
      return;
    }
    if (event.key === "ArrowUp") {
      event.preventDefault();
      void moveOmniboxSelection(-1);
      return;
    }
    if (event.key === "Escape") {
      event.preventDefault();
      clearSearchResults();
    }
  });

  window.addEventListener("keydown", (event) => {
    const metaOrCtrl = event.metaKey || event.ctrlKey;
    if (metaOrCtrl && event.key.toLowerCase() === "l") {
      event.preventDefault();
      focusAddressInput();
      return;
    }
    if (event.altKey && event.key === "ArrowLeft" && shouldHandleGlobalShortcut(event)) {
      event.preventDefault();
      void executeNavigationCommand("back", "Going back...");
      return;
    }
    if (event.altKey && event.key === "ArrowRight" && shouldHandleGlobalShortcut(event)) {
      event.preventDefault();
      void executeNavigationCommand("forward", "Going forward...");
    }
  });
}

async function loadInitialPageView() {
  const epoch = beginPageEpoch();
  try {
    await syncViewport(true, epoch);
    if (!isCurrentPageEpoch(epoch)) return;
    const view = await requestAppCommand<PageViewModel>({ type: "get_page_view" });
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
  networkLogEl = document.querySelector("#network-log");
  consoleLogEl = document.querySelector("#console-log");
  domSnapshotEl = document.querySelector("#dom-snapshot");
  crashReportEl = document.querySelector("#crash-report");
  securityInterstitialEl = document.querySelector("#security-interstitial");
  downloadsListEl = document.querySelector("#downloads-list");
  downloadSummaryEl = document.querySelector("#download-summary");
  downloadCurrentButtonEl = document.querySelector("#download-current-button");
  refreshDownloadsButtonEl = document.querySelector("#refresh-downloads-button");
  downloadDefaultDirInputEl = document.querySelector("#download-default-dir");
  downloadDefaultConfirmEl = document.querySelector("#download-default-confirm");
  downloadDefaultApplyButtonEl = document.querySelector("#download-default-apply");
  downloadSiteOriginInputEl = document.querySelector("#download-site-origin");
  downloadSiteDirInputEl = document.querySelector("#download-site-dir");
  downloadSiteConfirmEl = document.querySelector("#download-site-confirm");
  downloadSiteApplyButtonEl = document.querySelector("#download-site-apply");
  downloadSiteClearButtonEl = document.querySelector("#download-site-clear");
  downloadPolicyStatusEl = document.querySelector("#download-policy-status");

  document.querySelector("#url-form")?.addEventListener("submit", (event) => {
    event.preventDefault();
    void openCurrentInput();
  });
  backButtonEl?.addEventListener("click", () => void executeNavigationCommand("back", "Going back..."));
  forwardButtonEl?.addEventListener("click", () => void executeNavigationCommand("forward", "Going forward..."));
  reloadButtonEl?.addEventListener("click", () => void executeNavigationCommand("reload", "Reloading..."));
  newTabButtonEl?.addEventListener("click", () => void createTab());
  downloadCurrentButtonEl?.addEventListener("click", () => void enqueueCurrentUrlDownload());
  refreshDownloadsButtonEl?.addEventListener("click", () => void refreshDownloads());
  downloadDefaultApplyButtonEl?.addEventListener("click", () => void applyDefaultDownloadPolicy());
  downloadSiteApplyButtonEl?.addEventListener("click", () => void applySiteDownloadPolicy());
  downloadSiteClearButtonEl?.addEventListener("click", () => void clearSiteDownloadPolicy());
  downloadSiteOriginInputEl?.addEventListener("change", () => {
    if (latestDownloadPolicySettings) applyDownloadPolicyInputs(latestDownloadPolicySettings);
  });
  installKeyboardHandlers();

  if (viewportEl) {
    resizeObserver = new ResizeObserver(() => void syncViewport());
    resizeObserver.observe(viewportEl);
    viewportEl.addEventListener("scroll", () => scheduleScrollSync(), { passive: true });
  }

  window.addEventListener("beforeunload", () => {
    void flushScrollPositions();
    if (downloadPollTimer) window.clearInterval(downloadPollTimer);
  });
  downloadPollTimer = window.setInterval(() => void refreshDownloads(true), 1000);
  void refreshCrashReport();
  void refreshDownloads(true);
  void refreshDownloadPolicySettings(true);
  void loadInitialPageView();
});
