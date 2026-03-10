# Windows Portable Distribution

## Purpose
- This document describes the distributable artifact that can be shared with someone who does not have the source tree.
- The current deliverable is a portable Windows zip that contains the Tauri executable, a short usage note, and checksums.

## Build Script
- Script: `scripts/build-cosmobrowse-portable.ps1`
- Run from the repository root:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\build-cosmobrowse-portable.ps1
```

## What the Script Does
1. Build the frontend assets with `npm run build` in `saba/ui/cosmo-browse-ui`.
2. Build the Tauri desktop executable with `cargo build -p cosmo-browse-ui --release` in `saba`.
3. Copy the generated executable into `artifacts/cosmo-browse-ui/windows-portable/<version>/`.
4. Generate `README.txt`, `BUILD-INFO.txt`, and `SHA256SUMS.txt` next to the executable.
5. Create `artifacts/cosmo-browse-ui/cosmo-browse-ui-<version>-windows-portable.zip`.

## Output Layout
- Zip archive:
  - `artifacts/cosmo-browse-ui/cosmo-browse-ui-<version>-windows-portable.zip`
- Expanded folder:
  - `artifacts/cosmo-browse-ui/windows-portable/<version>/`
- Files inside the expanded folder:
  - `cosmo-browse-ui.exe`
  - `README.txt`
  - `BUILD-INFO.txt`
  - `SHA256SUMS.txt`

## Trial Instructions for a Non-Code Environment
1. Unzip the archive on a Windows machine.
2. Run `cosmo-browse-ui.exe`.
3. Enter `https://abehiroshi.la.coocan.jp/` in the URL field.
4. Verify left-menu navigation and one `_blank` external link.

## Runtime Prerequisite
- The Windows machine needs the Microsoft Edge WebView2 runtime, which is commonly preinstalled on current Windows 10 and Windows 11 environments.
- If the runtime is missing, install the Evergreen WebView2 runtime and rerun the executable.

## Current Scope
- This artifact is intended for quick trial distribution.
- It is not yet a signed installer and does not yet create Start Menu or Add/Remove Programs entries.
- If installer-grade distribution is required later, add a second path based on `tauri build` bundle outputs.
