#![no_std]

extern crate alloc;

pub use saba_core::*;

// Cosmic role aliases for gradual migration from legacy naming.
pub use saba_core::browser as orbit_engine;
pub use saba_core::renderer as nebula_renderer;
pub use saba_core::display_item as stardust_display;
