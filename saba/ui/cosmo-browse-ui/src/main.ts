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
type TabSummary = { id: number; title: string; url: string | null; is_active: boolean };
type SearchResult = { id: number; title: string; url: string; snippet: string };
type FrameScrollPositionSnapshot = { frame_id: string; position: ScrollPosition };
type AppError = { code: string; message: string; retryable: boolean };
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
  switch_tab: (payload) => legacyInvoke<PageViewModel>("switch_tab", { id: Number(payload?.id ?? 0) }),
  close_tab: (payload) => legacyInvoke<TabSummary[]>("close_tab", { id: Number(payload?.id ?? 0) }),
  list_tabs: () => legacyInvoke<TabSummary[]>("list_tabs"),
  search: (payload) => legacyInvoke<SearchResult[]>("search", { query: `${payload?.query ?? ""}` }),
  update_scroll_positions: (payload) =>
    legacyInvoke<boolean>("update_scroll_positions", {
      positions: Array.isArray(payload?.positions) ? payload.positions : [],
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
    releaseRolloutStatus = await invoke<ReleaseRolloutStatus>("release_rollout_status");
    activeTransport = releaseRolloutStatus.native_default_enabled ? nativeTransport : legacyTransport;
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
let resizeObserver: ResizeObserver | null = null;
let resizeTimer: number | null = null;
let scrollSyncTimer: number | null = null;
let pageEpoch = 0;

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
  };
  const base = descriptions[error.code] ? `${descriptions[error.code]} (${error.code})` : error.code;
  return `${base}: ${error.message}`;
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
  setStatus(formatError(appError), "error");
  maybeRenderSecurityInterstitial({ appError, requestedUrl });
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
    const report = await invokeNativeIpc<{ path: string; reason: string; crashed_at_ms: number; reproduction: string[] } | null>({ type: "get_latest_crash_report" });
    if (!crashReportEl) return;
    crashReportEl.textContent = report
      ? `Crash report: ${report.reason} @ ${new Date(report.crashed_at_ms).toISOString()} (${report.path})`
      : "Crash report: none";
  } catch {
    if (crashReportEl) crashReportEl.textContent = "Crash report: unavailable";
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

  if (isUrlLikeInput(value)) {
    await openUrl(value);
    return;
  }

  try {
    const results = await requestAppCommand<SearchResult[]>({ type: "search", payload: { query: value } });
    renderSearchResults(results);
    setStatus(`Found ${results.length} search results.`);
  } catch (errorValue) {
    handleCommandError(errorValue);
  }
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
    viewportEl.addEventListener("scroll", () => scheduleScrollSync(), { passive: true });
  }

  window.addEventListener("beforeunload", () => {
    void flushScrollPositions();
  });
  void refreshCrashReport();
  void loadInitialPageView();
});
