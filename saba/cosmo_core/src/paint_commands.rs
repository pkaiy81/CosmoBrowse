use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PaintCommand {
    DrawRect(DrawRect),
    DrawText(DrawText),
    DrawImage(DrawImage),
}

impl PaintCommand {
    pub fn fallback_text(
        x: i64,
        y: i64,
        text: impl Into<String>,
        color: String,
        font_px: i64,
        opacity: f64,
        href: Option<String>,
        z_index: i32,
        clip_rect: Option<(i64, i64, i64, i64)>,
    ) -> Self {
        Self::DrawText(DrawText {
            fixed: false,
            sticky: None,
            x,
            y,
            text: text.into(),
            color,
            font_px,
            font_family: String::from("monospace"),
            underline: false,
            bold: false,
            opacity,
            href,
            target: None,
            z_index,
            clip_rect,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PaintCommandList {
    pub commands: Vec<PaintCommand>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DrawRect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
    pub background_color: String,
    pub background_image: Option<String>,
    pub opacity: f64,
    pub z_index: i32,
    pub clip_rect: Option<(i64, i64, i64, i64)>,
    // The value of the element's HTML `id` attribute, when present.
    // Enables the renderer to resolve URL fragment anchors (#id) to a
    // pixel scroll offset without an additional DOM query.
    // Spec: HTML Living Standard §7.4 — scrolling to a fragment.
    // https://html.spec.whatwg.org/multipage/browsing-the-web.html#scroll-to-fragid
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_id: Option<String>,
    /// Border width in pixels (0 = no border).
    /// Set from the HTML `border` attribute on `<table>` (propagated to cells).
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub border_width: i64,
    /// CSS color string for the border (e.g. "#808080"). Empty when no border.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub border_color: String,
    /// CSS background-position as (x, x_is_percent, y, y_is_percent).
    /// Percentages resolve against (box − image) at paint time; pixel offsets
    /// may be negative (sprite sheets).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_position: Option<(f64, bool, f64, bool)>,
    /// `background-repeat: no-repeat` (false = repeat, the CSS default).
    #[serde(default, skip_serializing_if = "is_false")]
    pub background_no_repeat: bool,
    /// CSS background-size as (mode, w, w_is_percent, h, h_is_percent):
    /// mode 0 = explicit (negative dimension = auto), 1 = cover, 2 = contain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub background_size: Option<(u8, f64, bool, f64, bool)>,
    /// position:fixed — anchored to the viewport, exempt from scrolling.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fixed: bool,
    /// position:sticky context (top threshold, sticky box's laid-out y):
    /// the painter pins the subtree once scrolling passes the threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sticky: Option<(i64, i64, i64)>,
}

fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}

fn is_false(v: &bool) -> bool {
    !*v
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DrawText {
    /// position:fixed — exempt from scrolling.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fixed: bool,
    /// position:sticky context (top threshold, container y).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sticky: Option<(i64, i64, i64)>,
    pub x: i64,
    pub y: i64,
    pub text: String,
    pub color: String,
    pub font_px: i64,
    pub font_family: String,
    pub underline: bool,
    #[serde(default)]
    pub bold: bool,
    pub opacity: f64,
    pub href: Option<String>,
    pub target: Option<String>,
    pub z_index: i32,
    pub clip_rect: Option<(i64, i64, i64, i64)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DrawImage {
    /// position:fixed — exempt from scrolling.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fixed: bool,
    /// position:sticky context (top threshold, container y).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sticky: Option<(i64, i64, i64)>,
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
    pub src: String,
    pub alt: String,
    pub opacity: f64,
    pub href: Option<String>,
    pub target: Option<String>,
    pub z_index: i32,
    pub clip_rect: Option<(i64, i64, i64, i64)>,
}
