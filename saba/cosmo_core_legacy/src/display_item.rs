use crate::renderer::layout::computed_style::ComputedStyle;
use crate::renderer::layout::layout_object::LayoutPoint;
use crate::renderer::layout::layout_object::LayoutSize;
use alloc::string::String;

// Spec: Paint records map CSS visual formatting output into a backend-neutral display list.
// z-order follows CSS2 painting order + CSS Positioned Layout z-index buckets.
// clip_rect is a conservative hook for CSS Overflow clipping.

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PaintOrder {
    pub stacking_context: i32,
    pub z_index: i32,
}

impl PaintOrder {
    pub fn root() -> Self {
        Self {
            stacking_context: 0,
            z_index: 0,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct ClipRect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DisplayItem {
    Rect {
        style: ComputedStyle,
        layout_point: LayoutPoint,
        layout_size: LayoutSize,
        paint_order: PaintOrder,
        clip_rect: Option<ClipRect>,
        // The value of the element's `id` attribute, when present.
        // Used to resolve URL fragment anchors to a scroll offset.
        // Spec: HTML Living Standard §7.4 — navigating to a fragment.
        // https://html.spec.whatwg.org/multipage/browsing-the-web.html#scroll-to-fragid
        anchor_id: Option<String>,
    },
    Text {
        text: String,
        style: ComputedStyle,
        layout_point: LayoutPoint,
        href: Option<String>,
        // Spec: HTML Living Standard §4.6.21 — the `target` attribute on `<a>`.
        // https://html.spec.whatwg.org/multipage/links.html#attr-hyperlink-target
        target: Option<String>,
        paint_order: PaintOrder,
        clip_rect: Option<ClipRect>,
    },
    Image {
        src: String,
        alt: String,
        layout_point: LayoutPoint,
        layout_size: LayoutSize,
        style: ComputedStyle,
        href: Option<String>,
        // Spec: HTML Living Standard §4.6.21 — the `target` attribute on `<a>`.
        // https://html.spec.whatwg.org/multipage/links.html#attr-hyperlink-target
        target: Option<String>,
        paint_order: PaintOrder,
        clip_rect: Option<ClipRect>,
    },
}
