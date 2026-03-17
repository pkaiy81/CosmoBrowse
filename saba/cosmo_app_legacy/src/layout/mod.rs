use crate::model::{
    ContentSize, FrameRect, RenderBox, RenderNode, RenderNodeKind, RenderTreeSnapshot,
    ResolvedStyle, SceneItem,
};
use cosmo_core::nebula_renderer::css::cssom::CssParser;
use cosmo_core::nebula_renderer::css::token::CssTokenizer;
use cosmo_core::nebula_renderer::dom::api::{get_js_content, get_style_content};
use cosmo_core::nebula_renderer::dom::node::NodeKind;
use cosmo_core::nebula_renderer::html::parser::HtmlParser;
use cosmo_core::nebula_renderer::html::token::HtmlTokenizer;
use cosmo_core::nebula_renderer::js::ast::JsParser;
use cosmo_core::nebula_renderer::js::runtime::JsRuntime;
use cosmo_core::nebula_renderer::js::token::JsLexer;
use cosmo_core::nebula_renderer::layout::computed_style::{
    DisplayType, PositionType, TextDecoration,
};
use cosmo_core::nebula_renderer::layout::layout_object::{
    compute_box_model_metrics, LayoutObject, LayoutObjectKind,
};
use cosmo_core::nebula_renderer::layout::layout_view::LayoutView;
use cosmo_core::stardust_display::DisplayItem;
use std::cell::RefCell;
use std::rc::Rc;

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
    StyleChanged,
    IncrementalScenePatch,
}

impl RelayoutTrigger {
    pub fn as_diagnostic(&self) -> &'static str {
        match self {
            Self::ViewportChanged => "Relayout trigger: viewport changed",
            Self::DomChanged => "Relayout trigger: DOM changed",
            Self::StyleChanged => "Relayout trigger: style changed",
            Self::IncrementalScenePatch => "Relayout trigger: incremental scene patch",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutScene {
    pub scene_items: Vec<SceneItem>,
    pub content_size: ContentSize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScriptLayoutResult {
    pub layout_scene: LayoutScene,
    pub render_tree: RenderTreeSnapshot,
    pub diagnostics: Vec<String>,
    pub dom_updated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneDiffResult {
    pub added: Vec<SceneItem>,
    pub removed: Vec<SceneItem>,
    pub changed: Vec<SceneItem>,
}

impl SceneDiffResult {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// Strict relayout gate: only viewport/style/dom changes rebuild layout tree.
pub fn should_relayout(trigger: &RelayoutTrigger) -> bool {
    !matches!(trigger, RelayoutTrigger::IncrementalScenePatch)
}

pub fn diff_scene_items(previous: &[SceneItem], next: &[SceneItem]) -> SceneDiffResult {
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut changed = Vec::new();

    let shared = previous.len().min(next.len());
    for idx in 0..shared {
        if previous[idx] != next[idx] {
            changed.push(next[idx].clone());
        }
    }
    if next.len() > shared {
        added.extend_from_slice(&next[shared..]);
    }
    if previous.len() > shared {
        removed.extend_from_slice(&previous[shared..]);
    }

    SceneDiffResult {
        added,
        removed,
        changed,
    }
}

pub fn build_layout_scene_with_script_runtime(html: &str, rect: &FrameRect) -> ScriptLayoutResult {
    let tokenizer = HtmlTokenizer::new(html.to_string());
    let window = HtmlParser::new(tokenizer).construct_tree();
    let dom = window.borrow().document();

    let script = get_js_content(dom.clone());
    let mut runtime = JsRuntime::new(dom.clone());
    if !script.trim().is_empty() {
        let lexer = JsLexer::new(script);
        let mut parser = JsParser::new(lexer);
        let program = parser.parse_ast();
        runtime.execute(&program);
    }

    let style = get_style_content(dom.clone());
    let cssom = CssParser::new(CssTokenizer::new(style)).parse_stylesheet();
    let layout_view = LayoutView::new(dom, &cssom, rect.width.max(1));

    let layout_scene = display_items_to_scene(layout_view.paint(), rect);
    let render_tree = render_tree_snapshot(&layout_view, rect);
    let mut diagnostics = runtime.unsupported_apis();
    if runtime.dom_updated() {
        diagnostics.push("Render loop: DOM mutation -> relayout -> repaint".to_string());
    }

    ScriptLayoutResult {
        layout_scene,
        render_tree,
        diagnostics,
        dom_updated: runtime.dom_updated(),
    }
}

pub fn build_layout_scene(html: &str, rect: &FrameRect) -> LayoutScene {
    build_layout_scene_with_script_runtime(html, rect).layout_scene
}

fn display_items_to_scene(display_items: Vec<DisplayItem>, rect: &FrameRect) -> LayoutScene {
    let mut scene_items = Vec::with_capacity(display_items.len());
    let mut max_width = 0;
    let mut max_height = 0;

    for item in display_items {
        match item {
            DisplayItem::Rect {
                style,
                layout_point,
                layout_size,
                paint_order,
                clip_rect,
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
                    z_index: paint_order.z_index,
                    clip_rect: clip_rect.map(|c| (c.x, c.y, c.width, c.height)),
                });
            }
            DisplayItem::Text {
                text,
                style,
                layout_point,
                href,
                paint_order,
                clip_rect,
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
                    z_index: paint_order.z_index,
                    clip_rect: clip_rect.map(|c| (c.x, c.y, c.width, c.height)),
                });
            }
        }
    }

    scene_items.sort_by_key(|item| match item {
        SceneItem::Rect { z_index, .. }
        | SceneItem::Text { z_index, .. }
        | SceneItem::Image { z_index, .. } => *z_index,
    });

    LayoutScene {
        scene_items,
        content_size: ContentSize {
            width: max_width.max(rect.width),
            height: max_height.max(rect.height),
        },
    }
}

fn render_tree_snapshot(layout_view: &LayoutView, rect: &FrameRect) -> RenderTreeSnapshot {
    RenderTreeSnapshot {
        root: layout_view
            .root()
            .map(|node| layout_object_to_render_node(&node, rect)),
    }
}

fn layout_object_to_render_node(node: &Rc<RefCell<LayoutObject>>, rect: &FrameRect) -> RenderNode {
    let borrowed = node.borrow();
    let point = borrowed.point();
    let size = borrowed.size();
    let style = borrowed.style();
    let content_size = borrowed.content_size();

    let kind = match borrowed.kind() {
        LayoutObjectKind::Block => RenderNodeKind::Block,
        LayoutObjectKind::Inline => RenderNodeKind::Inline,
        LayoutObjectKind::Text => RenderNodeKind::Text,
    };

    let (node_name, text) = match borrowed.node_kind() {
        NodeKind::Document => ("#document".to_string(), None),
        NodeKind::Element(element) => (element.kind().to_string(), None),
        NodeKind::Text(value) => ("#text".to_string(), Some(value)),
    };

    let mut children = Vec::new();
    let mut child = borrowed.first_child();
    drop(borrowed);
    while let Some(current) = child {
        children.push(layout_object_to_render_node(&current, rect));
        child = current.borrow().next_sibling();
    }

    let box_model = compute_box_model_metrics(&style);

    RenderNode {
        kind,
        node_name,
        text,
        box_info: RenderBox {
            x: rect.x + point.x(),
            y: rect.y + point.y(),
            width: size.width(),
            height: size.height(),
            content_width: content_size.width(),
            content_height: content_size.height(),
            margin: (
                box_model.margin.top,
                box_model.margin.right,
                box_model.margin.bottom,
                box_model.margin.left,
            ),
            padding: (
                box_model.padding.top,
                box_model.padding.right,
                box_model.padding.bottom,
                box_model.padding.left,
            ),
            border: (
                box_model.border.top,
                box_model.border.right,
                box_model.border.bottom,
                box_model.border.left,
            ),
        },
        style: ResolvedStyle {
            display: match style.display() {
                DisplayType::Block => "block",
                DisplayType::Inline => "inline",
                DisplayType::DisplayNone => "none",
            }
            .to_string(),
            position: match style.position() {
                PositionType::Static => "static",
                PositionType::Relative => "relative",
                PositionType::Absolute => "absolute",
            }
            .to_string(),
            color: style.color().code().to_string(),
            background_color: style.background_color().code().to_string(),
            font_px: style.font_size().px(),
            font_family: style.font_family(),
            opacity: style.opacity(),
            z_index: style.z_index(),
        },
        children,
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

#[cfg(test)]
mod diff_tests {
    use super::*;

    #[test]
    fn diff_scene_items_detects_changed_rows() {
        let prev = vec![SceneItem::Rect {
            x: 0,
            y: 0,
            width: 10,
            height: 10,
            background_color: "#fff".to_string(),
            opacity: 1.0,
            z_index: 0,
            clip_rect: None,
        }];
        let next = vec![SceneItem::Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 10,
            background_color: "#fff".to_string(),
            opacity: 1.0,
            z_index: 1,
            clip_rect: None,
        }];
        let diff = diff_scene_items(&prev, &next);
        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        assert_eq!(diff.changed.len(), 1);
    }
}
