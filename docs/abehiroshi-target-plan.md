# Abe Hiroshi Target Plan

## Goal
- Open `https://abehiroshi.la.coocan.jp/` directly in CosmoBrowse.
- Preserve the two-frame layout and the main left-menu to right-pane navigation.
- Keep outbound links usable when the site points to external HTTPS pages.

## Implemented Direction
- See `docs/cosmobrowse-browser-architecture.md` for the current end-to-end architecture, processing flow, and code map.
- See `docs/https-display-implementation.md` for the HTTPS-specific implementation details and code map.
- Use Rust to fetch, decode, and parse frame documents.
- Render leaf documents in the Tauri frontend through `iframe srcdoc` with injected `<base>` and link interception.
- Keep history at the tab level so back/forward restores frame state snapshots.
- Delegate `_blank` and `mailto:` links to the host OS through the Tauri opener plugin.

## Current Acceptance Focus
- HTTPS fetch works.
- Shift_JIS HTML decodes correctly.
- `frameset/frame/noframes` pages split into recursive frames.
- Named frame navigation like `target="right"` updates only that frame.
- External links such as the live VIVANT link open outside CosmoBrowse instead of silently doing nothing.

## Delivery Status
- Done: HTTPS loading through Rust `reqwest` plus TLS error classification.
- Done: Shift_JIS decoding with diagnostics.
- Done: Recursive frameset parsing, nested framesets, and `noframes` fallback handling.
- Done: Targeted in-frame navigation for `right`, `_parent`, and `_top`.
- Done: `_blank` and `mailto:` outbound links now go through the Tauri opener path.

## Next Steps for External Outbound Navigation
1. Done: Confirm the live Abe Hiroshi external links use `target="_blank"`.
2. Done: Route `_blank` and `mailto:` leaf-frame clicks to the Tauri opener plugin.
3. Next: Add a dedicated regression fixture or frontend harness for outbound `_blank` navigation so this behavior is covered without relying only on manual live-site checks.
4. Next: Decide whether `_blank` should remain a host-browser escape hatch or evolve into an in-app tab/window model.
5. Later: Extend the same escape-hatch handling to scripted popup flows such as `window.open(...)` if a compatibility target requires it.

## Known Tradeoff
- Leaf document layout is delegated to the WebView engine instead of the legacy Rust scene layout. This is deliberate for the Abe Hiroshi compatibility target, where tables, body background attributes, and old HTML presentational markup are common.
- `_blank` currently opens in the system browser instead of a second in-app browsing context. This matches the current Tauri UI architecture and fixes the live external-link failure with minimal complexity.
