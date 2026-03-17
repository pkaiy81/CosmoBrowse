#![no_std]

extern crate alloc;

pub use cosmo_core_legacy::*;

// Cosmic role aliases for gradual migration from legacy naming.
pub use cosmo_core_legacy::browser as orbit_engine;
pub use cosmo_core_legacy::display_item as stardust_display;
pub use cosmo_core_legacy::renderer as nebula_renderer;

pub mod paint_mapper;

pub mod paint_commands;
