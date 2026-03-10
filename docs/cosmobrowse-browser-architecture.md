# CosmoBrowse Browser Architecture

## Purpose
- This document describes the current browsing pipeline used by CosmoBrowse, with emphasis on the Abe Hiroshi compatibility path.
- Update this document whenever the browsing flow, frame handling, rendering path, diagnostics, or verification tooling changes.

## Scope
- URL input, command dispatch, and Tauri bridge.
- Rust-side navigation state, history, fetching, decoding, and frame tree construction.
- Frontend-side frame rendering through `iframe srcdoc`.
- Frame-targeted navigation and verification tooling.

## High-Level Architecture
- Frontend UI: `saba/ui/cosmo-browse-ui/src/main.ts`
  - Collects user input, calls Tauri commands, renders the returned `PageViewModel`, and listens for embedded navigation messages from `iframe srcdoc` documents.
- Tauri command bridge: `saba/ui/cosmo-browse-ui/src-tauri/src/lib.rs`
  - Exposes `open_url`, `activate_link`, `reload`, `back`, `forward`, `set_viewport`, `get_page_view`, `list_tabs`, `new_tab`, `close_tab`, `search`, and `get_metrics`.
- Application service and navigation state: `saba/saba_app/src/session.rs`
  - Owns tabs, sessions, history, frame-target routing, page loading, and metrics.
- Fetch / decode / frame parsing helpers: `saba/saba_app/src/loader.rs`
  - Fetches documents, detects encoding, resolves URLs, parses `frameset` / `frame` / `noframes`, and injects `<base>` plus navigation script for leaf documents.
- Shared view models: `saba/saba_app/src/model.rs`
  - Defines `PageViewModel`, `FrameViewModel`, `NavigationState`, and metrics payloads.
- Inspection CLI: `saba/adapter_cli/src/main.rs`
  - Allows snapshot and metrics inspection outside the UI, including frame-targeted navigation replay.

## End-to-End Flow

### 1. User enters a URL or clicks a control
- Code:
  - `saba/ui/cosmo-browse-ui/src/main.ts`
- Main entry points:
  - `openCurrentInput()`
  - `openUrl()`
  - `executeNavigationCommand()`
  - `handleEmbeddedNavigation()`
- Behavior:
  - The UI determines whether the input is URL-like or should go through search.
  - For URLs, the UI calls a Tauri command through `invoke(...)`.

### 2. Tauri forwards the command into the Rust app state
- Code:
  - `saba/ui/cosmo-browse-ui/src-tauri/src/lib.rs`
- Behavior:
  - Tauri commands lock shared app state and call the `AppService` implementation.
  - The return value is serialized back to the frontend as JSON.

### 3. `SabaApp` dispatches to the active tab/session
- Code:
  - `saba/saba_app/src/session.rs`
- Main types:
  - `SabaApp`
  - `Tab`
  - `BrowserSession`
- Behavior:
  - `SabaApp` keeps multiple tabs and an active tab id.
  - `BrowserSession` owns the per-tab history stack and current viewport.
  - `execute_navigation(...)` wraps navigation calls and records timing / error metrics.

### 4. A page load starts from `BrowserSession::load_page`
- Code:
  - `saba/saba_app/src/session.rs`
- Main functions:
  - `BrowserSession::open_url(...)`
  - `BrowserSession::activate_link(...)`
  - `BrowserSession::load_page(...)`
  - `load_frame_recursive(...)`
- Behavior:
  - The session computes a root viewport rectangle.
  - The root document is loaded into a recursive frame tree.
  - The resulting tree becomes `PageViewModel.root_frame`.
- Page-level diagnostics are collected from the full frame tree and deduplicated before they are returned to the UI.

### 5. Documents are fetched and decoded
- Code:
  - `saba/saba_app/src/loader.rs`
- Main functions:
  - `fetch_document(...)`
  - `decode_html_bytes(...)`
  - `extract_title(...)`
  - `resolve_url(...)`
- Behavior:
  - Fixture documents are resolved first for tests.
  - Network documents are fetched through `reqwest`.
  - Encoding is chosen from HTTP `Content-Type`, then HTML meta charset, then UTF-8 fallback.
  - Diagnostics are appended when Shift_JIS decoding is used or replacement characters appear.

### 6. Frame documents are parsed into a recursive frame tree
- Code:
  - `saba/saba_app/src/loader.rs`
  - `saba/saba_app/src/session.rs`
- Main functions and types:
  - `parse_frameset_document(...)`
  - `parse_frameset_at(...)`
  - `FramesetSpec`
  - `FramesetChild`
  - `build_frameset_view(...)`
  - `build_inline_frameset_view(...)`
- Behavior:
  - The loader parses inline nested `frameset` structures recursively.
  - `noframes` fallback HTML is retained on the parsed spec.
  - Session code converts the parsed spec into nested `FrameViewModel` nodes.
  - External child frames recurse through `load_frame_recursive(...)`.
  - Inline nested framesets stay in the same document URL and only subdivide the rectangle tree.
  - When a `frameset` has no usable child frames but has `noframes`, the fallback is rendered as a leaf document.

### 7. Leaf documents are converted for WebView rendering
- Code:
  - `saba/saba_app/src/loader.rs`
  - `saba/saba_app/src/session.rs`
- Main functions:
  - `prepare_html_for_display(...)`
  - `build_leaf_frame_view(...)`
- Behavior:
  - A `<base>` element is injected so relative URLs resolve against the loaded document URL.
  - A small click interception script is injected into the leaf HTML.
  - The script posts `cosmobrowse:navigate` messages to the parent window when links are activated.
  - Head injection is case-insensitive and also works when the source document has no `<head>` element.

### 8. The frontend renders the returned frame tree
- Code:
  - `saba/ui/cosmo-browse-ui/src/main.ts`
  - `saba/ui/cosmo-browse-ui/src/styles.css`
- Main functions:
  - `renderPageView(...)`
  - `renderFrameTree(...)`
  - `renderFrame(...)`
- Behavior:
  - Each `FrameViewModel` becomes an absolutely positioned DOM section.
  - Container frames render nested children.
  - Leaf frames render an `<iframe>` whose `srcdoc` is the prepared HTML payload.

### 9. Clicking a link inside a frame triggers targeted navigation
- Code:
  - `saba/ui/cosmo-browse-ui/src/main.ts`
  - `saba/saba_app/src/session.rs`
- Main functions:
  - `handleEmbeddedNavigation(...)`
  - `BrowserSession::activate_link(...)`
  - `resolve_destination_frame_id(...)`
  - `find_frame_id_by_name(...)`
- Behavior:
  - The frontend receives the `postMessage(...)` payload from the leaf iframe.
  - `_blank` is opened in a new window by the frontend.
  - Other targets go back to Rust through `activate_link`.
  - `_self`, `_parent`, `_top`, and named frame targets are resolved in the session.
  - The session rebuilds the page using stored frame URL overrides so only the destination frame changes.

### 10. History and viewport changes re-render the current page
- Code:
  - `saba/saba_app/src/session.rs`
  - `saba/ui/cosmo-browse-ui/src/main.ts`
- Main functions:
  - `BrowserSession::back(...)`
  - `BrowserSession::forward(...)`
  - `BrowserSession::reload(...)`
  - `BrowserSession::set_viewport(...)`
  - `syncViewport(...)`
- Behavior:
  - History is stored at the tab/session level as full page snapshots.
  - Resizing the viewport causes a full page reload using the latest frame URL overrides.

## Core Data Structures
- `PageViewModel`: `saba/saba_app/src/model.rs`
  - The root payload returned to the UI.
  - Contains `current_url`, `title`, `diagnostics`, `content_size`, and `root_frame`.
- `FrameViewModel`: `saba/saba_app/src/model.rs`
  - Represents a single frame node.
  - A frame is either a container (`child_frames` non-empty) or a leaf (`html_content` present).
- `FramesetSpec`: `saba/saba_app/src/loader.rs`
  - Parsed legacy frameset structure before it becomes a view model tree.
- `FramesetChild`: `saba/saba_app/src/loader.rs`
  - Distinguishes leaf `frame` elements from nested inline `frameset` elements.

## Verification / Debugging Paths
- Fixture coverage:
  - `saba/testdata/abehiroshi/`
  - `saba/testdata/legacy_frames/`
- CLI commands:
  - `cargo run -p adapter_cli --offline -- get-snapshot <url>`
  - `cargo run -p adapter_cli --offline -- activate-link <url> <frame-id> <href> [target]`
  - `cargo run -p adapter_cli --offline -- metrics <url>`
- Current regression tests:
  - `saba/saba_app/src/loader.rs`
  - `saba/saba_app/src/session.rs`

## Current Tradeoffs
- Leaf document layout is delegated to the WebView engine instead of the legacy Rust layout engine.
- History stores complete page snapshots instead of diff-based updates.
- Link interception currently handles click-based anchor navigation, not arbitrary JavaScript-driven navigation.
- Page-level diagnostics are aggregated from all frames and deduplicated in first-seen order.

## Files That Commonly Need Joint Updates
- Frame parsing changes:
  - `saba/saba_app/src/loader.rs`
  - `saba/saba_app/src/session.rs`
  - `saba/testdata/legacy_frames/*`
- View model shape changes:
  - `saba/saba_app/src/model.rs`
  - `saba/ui/cosmo-browse-ui/src/main.ts`
  - `saba/ui/cosmo-browse-ui/src-tauri/src/lib.rs`
- Navigation behavior changes:
  - `saba/saba_app/src/session.rs`
  - `saba/ui/cosmo-browse-ui/src/main.ts`
  - `saba/adapter_cli/src/main.rs`

## Update Checklist
- If the browsing/rendering pipeline changes, update the relevant section in this document.
- If a new command or verification path is added, update `Verification / Debugging Paths`.
- If frame parsing or navigation semantics change, update both `End-to-End Flow` and `Current Tradeoffs`.
- If new fixtures or tests are added, list them in the verification section.