# Cosmic Naming Migration Plan

## Goal
CosmoBrowse gradually replaces legacy `saba_*` naming with product-aligned cosmic names while keeping the codebase buildable at every step.

## Current compatibility layer
- `cosmo_runtime` crate (wrapper over `saba_app`) provides app-layer naming for adapters.
- `cosmo_core` crate (wrapper over `saba_core`) provides engine-layer naming for render/network/parser components.

## Role mapping (legacy -> cosmic)
- `saba_core` -> `cosmo_core` (engine primitives)
  - `browser` -> `orbit_engine`
  - `renderer` -> `nebula_renderer`
  - `display_item` -> `stardust_display`
- `saba_app` -> `cosmo_runtime` (orchestration/runtime)
  - `SabaApp` -> `StarshipApp`
  - `PageViewModel` -> `OrbitSnapshot`
  - `FrameViewModel` -> `GalaxyFrame`

## Step-by-step rollout
1. Add wrapper crates and aliases (done in this phase).
2. Migrate adapter crates (`adapter_cli`, Tauri UI) to `cosmo_runtime` imports.
3. Migrate additional crates to `cosmo_core`/`cosmo_runtime` imports.
4. Rename package names on disk only after all internal imports are migrated.
5. Remove compatibility wrappers when no `saba_*` direct import remains.

## Why gradual migration
Large direct package renames break workspace references and downstream tooling. This staged approach keeps PR size reviewable and prevents integration downtime.
