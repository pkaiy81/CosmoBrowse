# Abe Hiroshi Target Plan

## Goal
- Open `https://abehiroshi.la.coocan.jp/` directly in CosmoBrowse.
- Preserve the two-frame layout and the main left-menu to right-pane navigation.

## Implemented Direction
- See docs/cosmobrowse-browser-architecture.md for the current end-to-end architecture, processing flow, and code map. Update it whenever the implementation changes.
- Use Rust to fetch/decode HTML and build the frame tree.
- Render leaf documents in the Tauri frontend through `iframe srcdoc` with injected `<base>` and link interception.
- Keep history at the tab level so back/forward restores frame state snapshots.

## Current Acceptance Focus
- HTTPS fetch works.
- Shift_JIS HTML decodes correctly.
- `frameset/frame/noframes` pages split into recursive frames.
- Named frame navigation like `target="right"` updates only that frame.

## Known Tradeoff
- Leaf document layout is delegated to the WebView engine instead of the legacy Rust scene layout. This is deliberate for the Abe Hiroshi compatibility target, where tables, body background attributes, and old HTML presentational markup are common.

