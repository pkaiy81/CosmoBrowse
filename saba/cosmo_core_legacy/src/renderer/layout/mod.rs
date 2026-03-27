pub mod computed_style;
pub mod layout_object;
pub mod layout_view;

/// CSS layout support contract for the current engine iteration.
///
/// Priority order (high -> low):
/// 1) block/inline normal flow
/// 2) positioned boxes (relative/absolute)
/// 3) stacking-context aware paint ordering
/// 4) clipping/compositing hooks
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutSupportProfile {
    pub block_inline_normal_flow: bool,
    pub relative_positioning: bool,
    pub absolute_positioning: bool,
    pub z_order_stacking: bool,
    pub clipping: bool,
    pub incremental_relayout: bool,
}

impl LayoutSupportProfile {
    pub const fn current() -> Self {
        Self {
            block_inline_normal_flow: true,
            relative_positioning: true,
            absolute_positioning: true,
            z_order_stacking: true,
            clipping: true,
            incremental_relayout: true,
        }
    }
}
