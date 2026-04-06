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
            x,
            y,
            text: text.into(),
            color,
            font_px,
            font_family: String::from("monospace"),
            underline: false,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DrawText {
    pub x: i64,
    pub y: i64,
    pub text: String,
    pub color: String,
    pub font_px: i64,
    pub font_family: String,
    pub underline: bool,
    pub opacity: f64,
    pub href: Option<String>,
    pub target: Option<String>,
    pub z_index: i32,
    pub clip_rect: Option<(i64, i64, i64, i64)>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DrawImage {
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
