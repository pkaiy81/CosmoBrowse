use crate::model::{
    ContentSize, FrameRect, RenderBox, RenderNode, RenderNodeKind, RenderTreeSnapshot,
    ResolvedStyle, SceneItem,
};
use crate::security::{local_storage_snapshot, replace_local_storage};
use cosmo_engine::js_runtime::JsDomRuntimeBridge;
use cosmo_engine::renderer::css::cssom::CssParser;
use cosmo_engine::renderer::css::token::CssTokenizer;
use crate::loader::fetch_external_stylesheets;
use cosmo_engine::renderer::dom::api::{
    get_js_content, get_style_content, get_stylesheet_links,
};
use cosmo_engine::renderer::dom::node::NodeKind;
use cosmo_engine::renderer::html::parser::HtmlParser;
use cosmo_engine::renderer::html::token::HtmlTokenizer;
use cosmo_engine::renderer::js::ast::JsParser;
use cosmo_engine::renderer::js::runtime::JsRuntime;
use cosmo_engine::renderer::js::token::JsLexer;
use cosmo_engine::renderer::layout::computed_style::{
    DisplayType, PositionType, TextDecoration,
};
use cosmo_engine::renderer::layout::layout_object::{
    compute_box_model_metrics, LayoutObject, LayoutObjectKind,
};
use cosmo_engine::renderer::layout::layout_view::LayoutView;
use cosmo_engine::display_item::DisplayItem;
use std::cell::RefCell;
use std::rc::Rc;

/// Re-layout triggers used by the app layer when deciding whether the scene tree must be rebuilt.
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

pub fn build_layout_scene_with_script_runtime(
    document_url: &str,
    html: &str,
    rect: &FrameRect,
) -> ScriptLayoutResult {
    let tokenizer = HtmlTokenizer::new(html.to_string());
    let window = HtmlParser::new(tokenizer).construct_tree();
    let dom = window.borrow().document();

    let script = get_js_content(dom.clone());
    let mut runtime = JsRuntime::new(dom.clone());
    runtime.replace_local_storage_entries(local_storage_snapshot(document_url));
    // Real-world pages (Wix, Squarespace, GA/GTM-instrumented sites) ship
    // hundreds of kilobytes of minified JavaScript that this engine cannot
    // meaningfully execute.  Even just *parsing* that volume of tokens can
    // take many seconds and trip an infinite loop in the recursive-descent
    // parser when fed constructs it doesn't understand.  Bail out early when
    // the script payload exceeds a conservative size so navigation to such
    // pages stays responsive — the engine doesn't run their JS anyway.
    const MAX_SCRIPT_BYTES: usize = 32 * 1024;
    if !script.trim().is_empty() && script.len() <= MAX_SCRIPT_BYTES {
        let lexer = JsLexer::new(script);
        let mut parser = JsParser::new(lexer);
        let program = parser.parse_ast();
        runtime.execute(&program);
    }
    replace_local_storage(document_url, &runtime.local_storage_entries());

    // Combine external <link rel="stylesheet"> sheets with inline <style>.
    // External sheets are applied first so a later inline <style> wins on equal
    // specificity (approximating document order). Fetching is cached by URL so
    // relayout does not hit the network again.
    // Spec: CSS Cascading §6 — declaration order within the same origin.
    // https://www.w3.org/TR/css-cascade-4/#cascade-order
    let links = get_stylesheet_links(dom.clone());
    let external_css = fetch_external_stylesheets(document_url, &links);
    let inline_css = get_style_content(dom.clone());
    let style = if external_css.is_empty() {
        inline_css
    } else {
        format!("{external_css}\n{inline_css}")
    };
    let cssom = CssParser::new(CssTokenizer::new(style)).parse_stylesheet();
    // var(--token) references are resolved per element during the cascade
    // (custom properties inherit; the document root seeds from the whole
    // stylesheet), so no global pre-substitution is needed here.
    let layout_view =
        LayoutView::new_with_viewport(dom, &cssom, rect.width.max(1), rect.height.max(0));

    let layout_scene = display_items_to_scene(layout_view.paint(), rect);
    let render_tree = render_tree_snapshot(&layout_view, rect);
    let mut diagnostics = runtime.diagnostics();
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
    build_layout_scene_with_script_runtime("about:blank", html, rect).layout_scene
}


/// Apply a stamped scale context to a layout point (page coordinates).
fn scaled_point(ctx: Option<(f64, f64, f64)>, x: i64, y: i64) -> (i64, i64) {
    match ctx {
        Some((ox, oy, s)) => (
            (ox + (x as f64 - ox) * s) as i64,
            (oy + (y as f64 - oy) * s) as i64,
        ),
        None => (x, y),
    }
}

/// Apply a stamped scale context to a length.
fn scaled_len(ctx: Option<(f64, f64, f64)>, v: i64) -> i64 {
    match ctx {
        Some((_, _, s)) => (v as f64 * s) as i64,
        None => v,
    }
}

/// Rotate a point about a rotation context's center (degrees, clockwise) so
/// text/image anchors travel with a rotated box (glyphs stay upright).
fn rotated_point(ctx: Option<(f64, f64, f64)>, x: i64, y: i64) -> (i64, i64) {
    match ctx {
        Some((cx, cy, deg)) => {
            let r = deg * std::f64::consts::PI / 180.0;
            let (sin, cos) = (r.sin(), r.cos());
            let dx = x as f64 - cx;
            let dy = y as f64 - cy;
            ((cx + dx * cos - dy * sin) as i64, (cy + dx * sin + dy * cos) as i64)
        }
        None => (x, y),
    }
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
                paint_order: _,
                clip_rect,
                anchor_id,
            } => {
                let ctx = style.scale_context();
                let (lx, ly) = scaled_point(ctx, layout_point.x(), layout_point.y());
                let (lw, lh) = (
                    scaled_len(ctx, layout_size.width()),
                    scaled_len(ctx, layout_size.height()),
                );
                let x = rect.x + lx;
                let y = rect.y + ly;
                max_width = max_width.max(lx + lw);
                max_height = max_height.max(ly + lh);
                let border = style.border_or_zero();
                let border_width = border.top()
                    .max(border.right())
                    .max(border.bottom())
                    .max(border.left())
                    .round() as i64;
                let border_widths = Some((
                    border.top().round() as i64,
                    border.right().round() as i64,
                    border.bottom().round() as i64,
                    border.left().round() as i64,
                ));
                let border_color = style.border_color()
                    .map(|c| c.code().to_string())
                    .unwrap_or_default();
                scene_items.push(SceneItem::Rect {
                    x,
                    y,
                    width: lw,
                    height: lh,
                    background_color: style.background_color().code().to_string(),
                    background_image: style.background_image().map(|s| s.to_string()),
                    opacity: style.opacity(),
                    // Final paint-order key from the engine's stacking pass
                    // (root canvas −2M, normal flow 0, contexts ±1M+z).
                    z_index: style.paint_z(),
                    clip_rect: clip_rect.map(|c| (c.x + rect.x, c.y + rect.y, c.width, c.height)),
                    anchor_id,
                    border_width,
                    border_widths,
                    border_color,
                    background_position: style.background_position(),
                    background_no_repeat: style.background_no_repeat(),
                    background_size: style.background_size(),
                    border_radius: scaled_len(ctx, style.border_radius() as i64),
                    box_shadow: style.box_shadow().map(|(dx, dy, b, c)| (dx as i64, dy as i64, b as i64, c.code().to_string())),
                    rotate: style.rotate_context().map(|(cx, cy, deg)| {
                        (rect.x + cx as i64, rect.y + cy as i64, deg)
                    }),
                    fixed: style.position() == PositionType::Fixed || style.fixed_subtree(),
                    sticky: style.sticky_context().map(|(t, y, m)| (t as i64, y as i64, m.min(i64::MAX as f64) as i64)),
                    scroll_container: style.scroll_container(),
                    scroll_container_def: style.scroll_container_def().map(|(i, w, h)| (i, w as i64, h as i64)),
                });
            }
            DisplayItem::Text {
                text,
                style,
                layout_point,
                href,
                target,
                paint_order: _,
                clip_rect,
                bold,
            } => {
                let ctx = style.scale_context();
                let (lx, ly) = scaled_point(ctx, layout_point.x(), layout_point.y());
                let (lx, ly) = rotated_point(style.rotate_context(), lx, ly);
                let x = rect.x + lx;
                let y = rect.y + ly;
                let font_px_scaled = scaled_len(ctx, style.font_size().px()).max(1);
                let width_estimate = text.len() as i64 * 8 * (font_px_scaled / 16).max(1);
                let height_estimate = font_px_scaled + 4;
                max_width = max_width.max(lx + width_estimate);
                max_height = max_height.max(ly + height_estimate);
                scene_items.push(SceneItem::Text {
                    fixed: style.position() == PositionType::Fixed || style.fixed_subtree(),
                    sticky: style.sticky_context().map(|(t, y, m)| (t as i64, y as i64, m.min(i64::MAX as f64) as i64)),
                    scroll_container: style.scroll_container(),
                    scroll_container_def: style.scroll_container_def().map(|(i, w, h)| (i, w as i64, h as i64)),
                    x,
                    y,
                    text,
                    color: style.color().code().to_string(),
                    font_px: font_px_scaled,
                    font_family: style.font_family(),
                    underline: style.text_decoration() == TextDecoration::Underline,
                    bold,
                    opacity: style.opacity(),
                    href,
                    target,
                    // Final paint-order key from the engine's stacking pass
                    // (root canvas −2M, normal flow 0, contexts ±1M+z).
                    z_index: style.paint_z(),
                    clip_rect: clip_rect.map(|c| (c.x + rect.x, c.y + rect.y, c.width, c.height)),
                });
            }
            DisplayItem::Image {
                src,
                alt,
                layout_point,
                layout_size,
                style,
                href,
                target,
                paint_order: _,
                clip_rect,
            } => {
                let ctx = style.scale_context();
                let (lx, ly) = scaled_point(ctx, layout_point.x(), layout_point.y());
                let (lw, lh) = (
                    scaled_len(ctx, layout_size.width()),
                    scaled_len(ctx, layout_size.height()),
                );
                let (lx, ly) = rotated_point(style.rotate_context(), lx, ly);
                let x = rect.x + lx;
                let y = rect.y + ly;
                max_width = max_width.max(lx + lw);
                max_height = max_height.max(ly + lh);
                scene_items.push(SceneItem::Image {
                    fixed: style.position() == PositionType::Fixed || style.fixed_subtree(),
                    sticky: style.sticky_context().map(|(t, y, m)| (t as i64, y as i64, m.min(i64::MAX as f64) as i64)),
                    scroll_container: style.scroll_container(),
                    scroll_container_def: style.scroll_container_def().map(|(i, w, h)| (i, w as i64, h as i64)),
                    x,
                    y,
                    width: lw,
                    height: lh,
                    src,
                    alt,
                    opacity: style.opacity(),
                    href,
                    target,
                    // Final paint-order key from the engine's stacking pass
                    // (root canvas −2M, normal flow 0, contexts ±1M+z).
                    z_index: style.paint_z(),
                    clip_rect: clip_rect.map(|c| (c.x + rect.x, c.y + rect.y, c.width, c.height)),
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
                DisplayType::InlineBlock => "inline-block",
                DisplayType::Flex => "flex",
                DisplayType::Grid => "grid",
                DisplayType::DisplayNone => "none",
            }
            .to_string(),
            position: match style.position() {
                PositionType::Static => "static",
                PositionType::Relative => "relative",
                PositionType::Absolute => "absolute",
                PositionType::Fixed => "fixed",
                PositionType::Sticky => "sticky",
            }
            .to_string(),
            color: style.color().code().to_string(),
            background_color: style.background_color().code().to_string(),
            font_px: style.font_size().px(),
            font_family: style.font_family(),
            opacity: style.opacity(),
            z_index: style.z_index_or_default(),
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
            background_image: None,
            opacity: 1.0,
            z_index: 0,
            clip_rect: None,
            anchor_id: None,
            border_width: 0,
            border_widths: None,
            border_color: String::new(),
            background_position: None,
            background_no_repeat: false,
            background_size: None,
            border_radius: 0,
            box_shadow: None,
            rotate: None,
            fixed: false,
            sticky: None,
            scroll_container: None,
            scroll_container_def: None,
        }];
        let next = vec![SceneItem::Rect {
            x: 0,
            y: 0,
            width: 20,
            height: 10,
            background_color: "#fff".to_string(),
            background_image: None,
            opacity: 1.0,
            z_index: 1,
            clip_rect: None,
            anchor_id: None,
            border_width: 0,
            border_widths: None,
            border_color: String::new(),
            background_position: None,
            background_no_repeat: false,
            background_size: None,
            border_radius: 0,
            box_shadow: None,
            rotate: None,
            fixed: false,
            sticky: None,
            scroll_container: None,
            scroll_container_def: None,
        }];
        let diff = diff_scene_items(&prev, &next);
        assert!(diff.added.is_empty());
        assert!(diff.removed.is_empty());
        assert_eq!(diff.changed.len(), 1);
    }
}
