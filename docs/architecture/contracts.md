# Foundation API Contracts (`saba_app`)

## Purpose
- Freeze minimal extension points in `saba_app` so callers can depend on stable contracts while backend internals evolve.
- Keep existing integrations backward-compatible by providing default implementations via `AppService`.

## New contracts

### `RenderBackend`
- Responsibility: expose a backend identity and map each `FrameViewModel` to a `RenderBackendKind`.
- Minimal surface:
  - `name(&self) -> &'static str`
  - `kind_for_frame(&self, frame: &FrameViewModel) -> RenderBackendKind`
- Default implementation: `DefaultRenderBackend`.
  - Returns `"webview"` for `name`.
  - Preserves existing behavior by forwarding the already-computed `frame.render_backend` hint.

### `ScriptEngine`
- Responsibility: expose script-engine capabilities without forcing script execution changes now.
- Minimal surface:
  - `name(&self) -> &'static str`
  - `can_execute(&self, frame: &FrameViewModel) -> bool` (default method)
- Default implementation: `DefaultScriptEngine`.
  - Returns `"disabled"` and `false` for execution capability.

### `SecurityPolicy`
- Responsibility: centralize policy decisions for navigation/content admission.
- Minimal surface:
  - `name(&self) -> &'static str`
  - `allows_navigation(&self, current_url: Option<&str>, target_url: &str) -> bool` (default method)
- Default implementation: `DefaultSecurityPolicy`.
  - Returns `"allow-navigation"` and allows navigation by default for compatibility.

## Backward compatibility strategy
- `AppService` now provides default trait methods:
  - `render_backend(&self) -> Box<dyn RenderBackend>`
  - `script_engine(&self) -> Box<dyn ScriptEngine>`
  - `security_policy(&self) -> Box<dyn SecurityPolicy>`
- Existing `AppService` implementers remain source-compatible because they can omit these methods.
- Existing runtime behavior remains unchanged because defaults intentionally reflect current behavior.

## Spec comment style
- Specification references in `saba_app` are now normalized to `// Spec:` comments.
- This unifies notation across loader/session code and improves scanability during standards audits.

## Migration guidance
- Current consumers can continue using `SabaApp` without code changes.
- Future backends/policies can incrementally override the new `AppService` methods and provide custom trait objects.

### `ScriptEngine` lifecycle integration (minimum runtime)
- `saba_app` now executes inline script during frame build and reflects DOM mutations in the same render cycle.
- Current minimum runtime scope:
  - click handlers (`onclick`, `addEventListener("click", ...)`)
  - timer task enqueue (`setTimeout`)
  - microtask enqueue (`queueMicrotask`)
  - DOM read/write (`document.getElementById`, `textContent`)
- When a script mutates DOM, diagnostics include a render-loop propagation marker and a relayout trigger.
- Unsupported browser APIs are reported with a unified message: `Unsupported browser API: <name>`.
- See `docs/architecture/js-event-loop.md` for the task/microtask processing diagram.


## Browser/Renderer IPC DTO (`cosmo-browse-ui`)
- Tauri IPC uses a fixed DTO: `BrowserPageDto`.
- The DTO includes:
  - `root_frame`: frame tree
  - `network_log`: HTTP/CORS/TLS diagnostic logs
  - `console_log`: script/DOM diagnostic logs
  - `dom_snapshot`: per-frame HTML snapshots
- Goal: prevent Browser internal model changes from cascading into the Renderer.
