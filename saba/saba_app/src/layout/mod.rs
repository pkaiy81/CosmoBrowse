use crate::model::{ContentSize, FrameRect, SceneItem};
use saba_core::display_item::DisplayItem;
use saba_core::renderer::css::cssom::CssParser;
use saba_core::renderer::css::token::CssTokenizer;
use saba_core::renderer::dom::api::get_style_content;
use saba_core::renderer::html::parser::HtmlParser;
use saba_core::renderer::html::token::HtmlTokenizer;
use saba_core::renderer::layout::computed_style::TextDecoration;
use saba_core::renderer::layout::layout_view::LayoutView;

/// Re-layout triggers used by `saba_app` when deciding whether the scene tree must be rebuilt.
///
/// Spec notes:
/// - DOM tree order: layout traversal consumes DOM nodes in tree order (pre-order), so trigger granularity is document/frame scoped.
/// - CSS2.2 visual formatting model: block/inline formatting and generated box dimensions depend on viewport and computed style.
/// - CSS positioning: positioned descendants may resolve offsets against containing blocks whose geometry changes on viewport updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayoutTrigger {
    ViewportChanged,
    DomChanged,
}

impl RelayoutTrigger {
    pub fn as_diagnostic(&self) -> &'static str {
        match self {
            Self::ViewportChanged => "Relayout trigger: viewport changed",
            Self::DomChanged => "Relayout trigger: DOM changed",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutScene {
    pub scene_items: Vec<SceneItem>,
    pub content_size: ContentSize,
}

pub fn build_layout_scene(html: &str, rect: &FrameRect) -> LayoutScene {
    let tokenizer = HtmlTokenizer::new(html.to_string());
    let window = HtmlParser::new(tokenizer).construct_tree();
    let dom = window.borrow().document();

    let style = get_style_content(dom.clone());
    let cssom = CssParser::new(CssTokenizer::new(style)).parse_stylesheet();
    let layout_view = LayoutView::new(dom, &cssom, rect.width.max(1));

    let display_items = layout_view.paint();
    let mut scene_items = Vec::with_capacity(display_items.len());
    let mut max_width = 0;
    let mut max_height = 0;

    for item in display_items {
        match item {
            DisplayItem::Rect {
                style,
                layout_point,
                layout_size,
            } => {
                let x = rect.x + layout_point.x();
                let y = rect.y + layout_point.y();
                max_width = max_width.max(layout_point.x() + layout_size.width());
                max_height = max_height.max(layout_point.y() + layout_size.height());
                scene_items.push(SceneItem::Rect {
                    x,
                    y,
                    width: layout_size.width(),
                    height: layout_size.height(),
                    background_color: style.background_color().code().to_string(),
                    opacity: style.opacity(),
                });
            }
            DisplayItem::Text {
                text,
                style,
                layout_point,
                href,
            } => {
                let x = rect.x + layout_point.x();
                let y = rect.y + layout_point.y();
                let width_estimate = text.len() as i64 * 8 * (style.font_size().px() / 16).max(1);
                let height_estimate = style.font_size().px() + 4;
                max_width = max_width.max(layout_point.x() + width_estimate);
                max_height = max_height.max(layout_point.y() + height_estimate);
                scene_items.push(SceneItem::Text {
                    x,
                    y,
                    text,
                    color: style.color().code().to_string(),
                    font_px: style.font_size().px(),
                    font_family: style.font_family(),
                    underline: style.text_decoration() == TextDecoration::Underline,
                    opacity: style.opacity(),
                    href,
                    target: None,
                });
            }
        }
    }

    LayoutScene {
        scene_items,
        content_size: ContentSize {
            width: max_width.max(rect.width),
            height: max_height.max(rect.height),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_layout_scene_offsets_by_frame_rect() {
        let rect = FrameRect {
            x: 32,
            y: 48,
            width: 400,
            height: 240,
        };
        let html = "<html><head><style>body{margin:0}p{margin:0}</style></head><body><p>Hello</p></body></html>";

        let scene = build_layout_scene(html, &rect);

        assert!(!scene.scene_items.is_empty());
        let first_x = match &scene.scene_items[0] {
            SceneItem::Rect { x, .. } => *x,
            SceneItem::Text { x, .. } => *x,
            SceneItem::Image { x, .. } => *x,
        };
        assert!(first_x >= rect.x);
    }
}
