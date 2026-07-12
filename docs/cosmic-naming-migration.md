# Cosmic Naming Migration Plan — COMPLETED (superseded)

**Status: completed 2026-07 with a simplification.** The legacy wrappers are gone
and the workspace uses plain crate/module paths. The fanciful cosmic *module
aliases* (`orbit_engine` / `nebula_renderer` / `stardust_display`,
`StarshipApp` / `OrbitSnapshot` / `GalaxyFrame`) were dropped instead of being
kept as a second naming layer — see ADR-0003.

## Final naming

| Old | Final |
|---|---|
| `saba_core` / `cosmo_core_legacy` / `cosmo_core` (shim) | `cosmo_engine` |
| `saba_app` / `cosmo_app_legacy` / `cosmo_runtime` (wrapper) | `cosmo_runtime` |
| `cosmo_core::orbit_engine` | `cosmo_engine::browser` |
| `cosmo_core::nebula_renderer` | `cosmo_engine::renderer` |
| `cosmo_core::stardust_display` | `cosmo_engine::display_item` |
| `SabaApp` / `StarshipApp` | `BrowserApp` |
| `OrbitSnapshot` | `PageViewModel` |
| `GalaxyFrame` | `FrameViewModel` |

The `cosmo_core` shim's real modules (`paint_commands`, `paint_mapper`,
`js_runtime`) moved into `cosmo_engine`. `scene_items_to_paint_commands` moved
from the wrapper into `cosmo_runtime::paint`.

## Enforcement

CI (`.github/workflows/ci.yml`, rust job) fails on any reference to
`saba_app` / `saba_core` / `cosmo_core_legacy` / `cosmo_app_legacy` /
`cosmo_core` outside Markdown/docs.
