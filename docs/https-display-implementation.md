# HTTPS Display Implementation

## Purpose
- This document explains the implementation that lets CosmoBrowse open and display HTTPS pages, with the Abe Hiroshi website as the compatibility target.
- Update this document whenever the HTTPS fetch path, decoding logic, frame loading, or frontend display bridge changes.

## Problem Statement
- The original Rust-only rendering path was not sufficient for modern HTTPS pages and legacy frame-heavy HTML at the same time.
- The Abe Hiroshi site requires all of the following in one flow: TLS-capable fetching, Shift_JIS decoding, `frameset/frame/noframes` parsing, relative URL resolution, and targeted navigation inside frames.
- CosmoBrowse now splits this work between Rust for loading/state management and the Tauri WebView for final leaf-document layout.

## Implementation Overview
1. The frontend accepts a URL and sends it to the Tauri command bridge.
2. Rust fetches the document over HTTP or HTTPS.
3. Rust decodes the response body using HTTP headers, HTML meta charset hints, and UTF-8 fallback.
4. Rust detects whether the document is a legacy `frameset` page or a leaf HTML page.
5. Rust builds a recursive frame tree and converts leaf HTML into `iframe srcdoc` payloads.
6. The frontend renders each frame rectangle and delegates leaf-document layout to the WebView engine.
7. Link clicks inside leaf frames are posted back to the parent app, which either routes the navigation in Rust or opens the destination externally.

## Code Map
- Frontend entry and rendering: `saba/ui/cosmo-browse-ui/src/main.ts`
  - `openUrl()`
  - `handleEmbeddedNavigation()`
  - `renderFrame()`
  - `syncViewport()`
- Tauri command bridge: `saba/ui/cosmo-browse-ui/src-tauri/src/lib.rs`
  - `open_url()`
  - `activate_link()`
  - `set_viewport()`
  - `tauri_plugin_opener::init()`
- Rust session and navigation state: `saba/saba_app/src/session.rs`
  - `BrowserSession::open_url(...)`
  - `BrowserSession::activate_link(...)`
  - `BrowserSession::load_page(...)`
  - `load_frame_recursive(...)`
  - `resolve_destination_frame_id(...)`
  - `normalize_target_keyword(...)`
- Rust loading helpers: `saba/saba_app/src/loader.rs`
  - `fetch_document(...)`
  - `decode_html_bytes(...)`
  - `resolve_url(...)`
  - `parse_frameset_document(...)`
  - `prepare_html_for_display(...)`

## Step 1: HTTPS Fetching in Rust
- File: `saba/saba_app/src/loader.rs`
- Function: `fetch_document(...)`
- Behavior:
  - Build a `reqwest::blocking::Client`.
  - Send a GET request to the requested URL.
  - Record the final URL after redirects.
  - Read the response body as bytes so charset detection can run before text conversion.
- Why this matters:
  - HTTPS support depends on `reqwest` and the Rust TLS stack, not on the old Wasabi networking path.
  - Redirect-aware final URLs are required so `<base>` injection and later relative-link resolution remain correct.
- Error handling:
  - `classify_request_error(...)` maps TLS and certificate failures into explicit app error categories.

## Step 2: Charset Detection and HTML Decoding
- File: `saba/saba_app/src/loader.rs`
- Function: `decode_html_bytes(...)`
- Behavior:
  - Read `charset=` from the HTTP `Content-Type` header when present.
  - If the header is absent or incomplete, sniff the HTML prefix for `<meta charset>` or legacy `http-equiv` declarations.
  - Fall back to UTF-8 when no explicit charset can be found.
  - Record diagnostics when Shift_JIS decoding is used or replacement characters appear.
- Relevant specs already referenced in code comments:
  - HTML Living Standard character encoding detection.
  - HTML Living Standard `meta charset` processing.
- Why this matters:
  - The Abe Hiroshi site serves Shift_JIS HTML, so incorrect decoding breaks titles, link labels, and diagnostics.

## Step 3: URL Resolution and Redirect Safety
- File: `saba/saba_app/src/loader.rs`
- Function: `resolve_url(...)`
- Behavior:
  - Resolve relative URLs against the current document URL with the `url` crate.
  - Preserve absolute `https://...` targets without rewriting them.
- Relevant spec already referenced in code comments:
  - RFC 3986 relative reference resolution.
- Why this matters:
  - Frame `src`, anchor `href`, and image URLs inside `srcdoc` documents all depend on correct base URL handling.

## Step 4: Legacy Frameset Parsing
- Files:
  - `saba/saba_app/src/loader.rs`
  - `saba/saba_app/src/session.rs`
- Functions and types:
  - `parse_frameset_document(...)`
  - `parse_frameset_at(...)`
  - `FramesetSpec`
  - `FramesetChild`
  - `BrowserSession::load_page(...)`
  - `load_frame_recursive(...)`
- Behavior:
  - Detect the first `<frameset>` in the loaded HTML.
  - Parse `rows` and `cols` track definitions.
  - Preserve nested inline `frameset` blocks recursively.
  - Preserve `noframes` fallback HTML for frameset documents that need a leaf fallback.
  - Build a nested `FrameViewModel` tree with stable frame ids and rectangles.
- Why this matters:
  - `https://abehiroshi.la.coocan.jp/` is not a single leaf document. The root page is a frameset that must keep left and right panes independent.

## Step 5: Leaf HTML Conversion for WebView Rendering
- File: `saba/saba_app/src/loader.rs`
- Function: `prepare_html_for_display(...)`
- Behavior:
  - Inject a `<base>` tag into each leaf HTML document.
  - Inject a small script that intercepts anchor clicks and posts `cosmobrowse:navigate` messages to the parent window.
  - Insert the payload before `</head>` case-insensitively.
  - If the document has no `<head>`, synthesize one after `<html>`.
- Relevant specs already referenced in code comments:
  - HTML Living Standard document base URL.
  - HTML Living Standard `iframe srcdoc` processing model.
- Why this matters:
  - Relative assets and links must keep working after the document is moved into `srcdoc`.
  - This bridge lets the app keep Rust-side frame history and targeted navigation semantics.

## Step 6: Session State, Targeted Navigation, and History
- File: `saba/saba_app/src/session.rs`
- Behavior:
  - `BrowserSession` stores per-tab history as complete page snapshots.
  - `activate_link(...)` resolves the clicked URL relative to the source frame.
  - Target keywords such as `_self`, `_parent`, `_top`, and `_blank` are normalized using ASCII case-insensitive handling for keyword values.
  - Named frame targets are resolved against the current frame tree.
  - Only the destination frame is reloaded when a named target points at an existing child frame.
- Why this matters:
  - Abe Hiroshi menu navigation depends on `target="right"` updating only the main pane.
  - Snapshot-based history lets back/forward restore the entire frame tree without diff reconstruction.

## Step 7: Frontend Rendering and External Link Escape Hatch
- File: `saba/ui/cosmo-browse-ui/src/main.ts`
- Behavior:
  - `renderFrame(...)` maps each `FrameViewModel` to an absolutely positioned DOM container.
  - Leaf documents are rendered with `iframe.srcdoc`.
  - `window.addEventListener("message", ...)` receives navigation messages from leaf frames.
  - `_blank` and `mailto:` links are delegated to the Tauri opener plugin so the host OS browser or mail client handles them.
  - All other navigations go back through the Rust `activate_link` command.
- Why this matters:
  - Tauri does not automatically create secondary browsing windows from `srcdoc` click interception.
  - Delegating external navigation to the host shell fixes live Abe Hiroshi links such as the VIVANT external site link.

## Diagnostics and Verification
- Unit tests:
  - `saba/saba_app/src/loader.rs`
  - `saba/saba_app/src/session.rs`
- Manual and CLI checks:
  - `cargo run -p adapter_cli --offline -- get-snapshot https://abehiroshi.la.coocan.jp/`
  - `cargo run -p adapter_cli --offline -- activate-link https://abehiroshi.la.coocan.jp/ root/left https://abehiroshi.la.coocan.jp/movie/eiga.htm right`
- UI verification:
  - Open the Abe Hiroshi top page in the Tauri UI.
  - Confirm left-to-right frame navigation still works.
  - Confirm external `_blank` links open in the system browser.

## Current Tradeoffs
- Leaf layout is still delegated to the WebView engine rather than the Rust scene engine.
- `_blank` links currently open outside the app instead of creating a second in-app browsing context.
- The click bridge handles anchor activation, not arbitrary JavaScript-driven popup flows.
- History uses full page snapshots, which is simple and reliable but not the most memory-efficient representation.

## Related Documents
- Architecture overview: `docs/cosmobrowse-browser-architecture.md`
- Abe Hiroshi delivery plan: `docs/abehiroshi-target-plan.md`
- Windows trial artifact flow: `docs/windows-portable-distribution.md`
