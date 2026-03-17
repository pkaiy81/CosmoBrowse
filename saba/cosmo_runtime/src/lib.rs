pub use cosmo_app_legacy::*;
use serde::{Deserialize, Serialize};

// Cosmic app-layer aliases.
pub type StarshipApp = SabaApp;
pub type OrbitSnapshot = PageViewModel;
pub type GalaxyFrame = FrameViewModel;

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
            font_family: "monospace".to_string(),
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
    pub opacity: f64,
    pub z_index: i32,
    pub clip_rect: Option<(i64, i64, i64, i64)>,
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


pub fn scene_items_to_paint_commands(scene_items: &[SceneItem]) -> (PaintCommandList, Vec<AppError>) {
    let mut commands = Vec::with_capacity(scene_items.len());
    let mut diagnostics = Vec::new();
    let mut errors = Vec::new();

    for item in scene_items {
        match item {
            SceneItem::Rect {
                x,
                y,
                width,
                height,
                background_color,
                opacity,
                z_index,
                clip_rect,
            } => commands.push(PaintCommand::DrawRect(DrawRect {
                x: *x,
                y: *y,
                width: *width,
                height: *height,
                background_color: background_color.clone(),
                opacity: *opacity,
                z_index: *z_index,
                clip_rect: *clip_rect,
            })),
            SceneItem::Text {
                x,
                y,
                text,
                color,
                font_px,
                font_family,
                underline,
                opacity,
                href,
                target,
                z_index,
                clip_rect,
            } => {
                let family = if font_family.trim().is_empty() {
                    diagnostics.push("Paint fallback: missing font-family, using monospace".to_string());
                    errors.push(AppError::state("font is unavailable; rendered with fallback monospace"));
                    "monospace".to_string()
                } else {
                    font_family.clone()
                };
                commands.push(PaintCommand::DrawText(DrawText {
                    x: *x,
                    y: *y,
                    text: text.clone(),
                    color: color.clone(),
                    font_px: *font_px,
                    font_family: family,
                    underline: *underline,
                    opacity: *opacity,
                    href: href.clone(),
                    target: target.clone(),
                    z_index: *z_index,
                    clip_rect: *clip_rect,
                }));
            }
            SceneItem::Image {
                x,
                y,
                width,
                height,
                src,
                alt,
                opacity,
                href,
                target,
                z_index,
                clip_rect,
            } => {
                // Spec note: HTML images should expose fallback text when image data is unavailable.
                // We preserve accessibility intent by painting a neutral placeholder and `alt` text.
                // Ref: HTML Living Standard, image fallback content and `alt` text behavior.
                if src.trim().is_empty() {
                    diagnostics.push("Paint fallback: image source missing, drawing placeholder".to_string());
                    errors.push(AppError::state("image resource is unavailable; rendered placeholder"));
                    commands.push(PaintCommand::DrawRect(DrawRect {
                        x: *x,
                        y: *y,
                        width: *width,
                        height: *height,
                        background_color: "#d0d0d0".to_string(),
                        opacity: *opacity,
                        z_index: *z_index,
                        clip_rect: *clip_rect,
                    }));
                    commands.push(PaintCommand::fallback_text(
                        *x + 4,
                        *y + 4,
                        if alt.is_empty() { "[image]" } else { alt },
                        "#444444".to_string(),
                        12,
                        *opacity,
                        href.clone(),
                        *z_index,
                        *clip_rect,
                    ));
                } else {
                    commands.push(PaintCommand::DrawImage(DrawImage {
                        x: *x,
                        y: *y,
                        width: *width,
                        height: *height,
                        src: src.clone(),
                        alt: alt.clone(),
                        opacity: *opacity,
                        href: href.clone(),
                        target: target.clone(),
                        z_index: *z_index,
                        clip_rect: *clip_rect,
                    }));
                }
            }
        }
    }

    (PaintCommandList { commands, diagnostics }, errors)
}
