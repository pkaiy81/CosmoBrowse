use crate::model::{
    ContentSize, FrameRect, RenderBox, RenderNode, RenderNodeKind, RenderTreeSnapshot,
    ResolvedStyle, SceneItem,
};
use crate::security::{local_storage_snapshot, replace_local_storage};
use cosmo_engine::renderer::css::cssom::CssParser;
use cosmo_engine::renderer::css::token::CssTokenizer;
use crate::loader::fetch_external_stylesheets;
use cosmo_engine::renderer::dom::api::{
    get_js_content, get_style_content, get_stylesheet_links,
};
use cosmo_engine::renderer::dom::node::NodeKind;
use cosmo_engine::renderer::html::parser::HtmlParser;
use cosmo_engine::renderer::html::token::HtmlTokenizer;
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

    // Script execution: the real Boa engine (cosmo_script) mutates `dom` in
    // place, so layout below sees the post-script tree.
    let (dom_updated, mut script_diagnostics) = execute_scripts_boa(document_url, dom.clone());

    let (layout_scene, render_tree) = layout_dom(dom, document_url, rect);
    let mut diagnostics = std::mem::take(&mut script_diagnostics);
    if dom_updated {
        diagnostics.push("Render loop: DOM mutation -> relayout -> repaint".to_string());
    }

    ScriptLayoutResult {
        layout_scene,
        render_tree,
        diagnostics,
        dom_updated,
    }
}

/// Parse `html` and lay it out **without running scripts**. Used by the
/// session to produce the initial frame structure/static content; the GUI's
/// `AppBridge` then owns script execution via a persistent [`LivePage`] (so
/// scripts run exactly once, on the renderer thread — Boa's Context is !Send
/// and can't live behind the adapter's Mutex). Non-GUI callers that want
/// scripts use [`build_layout_scene_with_script_runtime`] directly.
pub fn build_static_scene(document_url: &str, html: &str, rect: &FrameRect) -> ScriptLayoutResult {
    let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
    let dom = window.borrow().document();
    let (layout_scene, render_tree) = layout_dom(dom, document_url, rect);
    ScriptLayoutResult {
        layout_scene,
        render_tree,
        diagnostics: Vec::new(),
        dom_updated: false,
    }
}

/// Resolve styles for `dom` and produce its scene + render-tree snapshot at
/// `rect`. Shared by the one-shot pipeline and the persistent [`LivePage`], so
/// script-driven mutations re-layout identically.
fn layout_dom(
    dom: Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>,
    document_url: &str,
    rect: &FrameRect,
) -> (LayoutScene, RenderTreeSnapshot) {
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
    (layout_scene, render_tree)
}

/// A persistently-hosted page: keeps its Boa `ScriptHost` and DOM alive across
/// layout passes so asynchronous work (fetch/XHR/timers) can settle *after* the
/// first paint and drive an incremental re-layout — the basis for progressive
/// rendering. (Skeleton: the render loop still needs to be wired to call
/// `pump_and_relayout` when async work is pending, and to wake on completion.)
///
/// NB: each `ScriptHost` now owns its per-page state (plan D5 done), so
/// multiple `LivePage`s can coexist on one thread — the active one is swapped
/// in on each call. (AppBridge currently hosts only the root frame; framesets/
/// child frames could each get their own LivePage.)
pub struct LivePage {
    host: cosmo_script::ScriptHost,
    dom: Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>,
    document_url: String,
    /// DOM mutation generation at the last layout; a pump that leaves it
    /// unchanged skips re-layout (no script tick touched the DOM).
    last_generation: u64,
}

impl LivePage {
    /// Parse `html`, run its scripts once (immediate first paint — no waiting on
    /// in-flight fetches), and lay out. The host and DOM are retained so
    /// `pump_and_relayout` can apply later async mutations. When `waker` is
    /// provided, fetch completions call it so the render loop can wake and pump.
    pub fn load(
        document_url: &str,
        html: &str,
        rect: &FrameRect,
        waker: Option<crate::loader::FetchWaker>,
    ) -> (Self, LayoutScene) {
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let dom = window.borrow().document();

        let mut host = cosmo_script::ScriptHost::new();
        host.set_location(document_url);
        host.set_local_storage_entries(local_storage_snapshot(document_url));
        let engine = match waker {
            Some(w) => crate::loader::make_fetch_engine_with_waker(document_url, w),
            None => crate::loader::make_fetch_engine(document_url),
        };
        host.set_fetch_engine(engine);
        host.set_document(dom.clone());

        let script = get_js_content(dom.clone());
        const MAX_SCRIPT_BYTES: usize = 512 * 1024;
        if !script.trim().is_empty() && script.len() <= MAX_SCRIPT_BYTES {
            let _ = host.eval_to_string(&script);
            host.run_initial_load(1000);
        }
        replace_local_storage(document_url, &host.local_storage_entries());

        let (scene, _tree) = layout_dom(dom.clone(), document_url, rect);
        let last_generation = host.dom_generation();
        (
            Self {
                host,
                dom,
                document_url: document_url.to_string(),
                last_generation,
            },
            scene,
        )
    }

    /// Whether asynchronous work (fetch/XHR) is still outstanding — i.e. another
    /// `pump_and_relayout` may yield an updated scene.
    pub fn has_pending_work(&self) -> bool {
        self.host.has_pending_fetches()
    }

    /// Re-lay-out the retained DOM at `rect` **without** running scripts or
    /// pumping async work (used on viewport resize — a reflow, not a re-run).
    pub fn relayout(&mut self, rect: &FrameRect) -> LayoutScene {
        let (scene, _tree) = layout_dom(self.dom.clone(), &self.document_url, rect);
        scene
    }

    /// Drain any settled async work (fetch/XHR completions, timers) so their
    /// `.then`/handlers run. If that mutated the DOM (mutation generation
    /// changed), re-lay-out the retained DOM at `rect` and return the fresh
    /// scene; otherwise return `None` (nothing to repaint). Does not re-parse
    /// the HTML.
    pub fn pump_and_relayout(&mut self, rect: &FrameRect) -> Option<LayoutScene> {
        self.host.run_initial_load(1000);
        replace_local_storage(&self.document_url, &self.host.local_storage_entries());
        let generation = self.host.dom_generation();
        if generation == self.last_generation {
            return None;
        }
        self.last_generation = generation;
        let (scene, _tree) = layout_dom(self.dom.clone(), &self.document_url, rect);
        Some(scene)
    }

    /// Drain buffered `console.*` output (diagnostics).
    pub fn take_console_log(&self) -> Vec<String> {
        self.host.take_console_log()
    }
}

/// Run page scripts with the real Boa engine (`cosmo_script`). Mutates `dom`
/// in place. A byte cap remains as an interim watchdog: Boa has no fuel/
/// instruction budget in 0.20, so an unbounded minified bundle could hang the
/// pipeline (the same failure the toy path guards against).
fn execute_scripts_boa(
    document_url: &str,
    dom: Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>,
) -> (bool, Vec<String>) {
    let script = get_js_content(dom.clone());
    let mut diagnostics = Vec::new();
    let mut host = cosmo_script::ScriptHost::new();
    host.set_location(document_url);
    host.set_local_storage_entries(local_storage_snapshot(document_url));
    host.set_fetch_engine(crate::loader::make_fetch_engine(document_url));
    host.set_document(dom);

    // Interim watchdog (see fn doc). Larger than the toy cap since Boa handles
    // far more real-world JS, but still bounded.
    const MAX_SCRIPT_BYTES: usize = 512 * 1024;
    let ran = if !script.trim().is_empty() && script.len() <= MAX_SCRIPT_BYTES {
        if let Err(e) = host.eval_to_string(&script) {
            diagnostics.push(format!("Script error: {e}"));
        }
        // Drain microtasks + due one-shot timers as at initial load; each
        // interval fires at most once (no spinning at first paint). Bounded to
        // cap runaway setTimeout(0) chains.
        host.run_initial_load(1000);
        // Interim lifecycle: this layout pass is one-shot, so wait (bounded)
        // for in-flight fetch() requests to settle and their .then chains to
        // mutate the DOM before we lay out. Progressive rendering across
        // passes (paint, then update on completion) is future work — see
        // HANDOFF. The IO itself runs on worker threads, so this only blocks
        // on genuinely slow responses up to the deadline.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while host.has_pending_fetches() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
            host.run_initial_load(1000);
        }
        true
    } else {
        false
    };
    replace_local_storage(document_url, &host.local_storage_entries());
    for line in host.take_console_log() {
        diagnostics.push(format!("console: {line}"));
    }
    // Without a mutation-generation counter we can't cheaply tell whether the
    // DOM actually changed; conservatively relayout whenever a script ran.
    (ran, diagnostics)
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
                    background_gradient: style.background_gradient().map(|g| {
                        (
                            g.angle_deg,
                            g.stops
                                .iter()
                                .map(|(c, p)| (c.code().to_string(), *p))
                                .collect(),
                        )
                    }),
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
                DisplayType::Contents => "contents",
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

    #[test]
    fn boa_path_executes_scripts_and_mutates_dom() {
        // The real Boa engine (cosmo_script) runs a script that appends a DOM
        // node; the mutation must be visible in the resulting layout scene.
        let html = "<html><head><style>body{margin:0}</style></head><body>\
            <ul id=\"list\"></ul>\
            <script>\
                var li = document.createElement('li'); \
                li.textContent = 'from-js'; \
                document.getElementById('list').appendChild(li); \
                console.log('ran');\
            </script></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let dom = window.borrow().document();

        let (dom_updated, diagnostics) = execute_scripts_boa("about:blank", dom.clone());
        assert!(dom_updated);
        assert!(
            diagnostics.iter().any(|d| d.contains("ran")),
            "console output should be captured: {diagnostics:?}"
        );

        // The appended text is now part of the document.
        let mut text = String::new();
        cosmo_engine::renderer::dom::api::collect_text(Some(dom), &mut text);
        assert!(text.contains("from-js"), "DOM mutation not applied: {text:?}");
    }

    #[test]
    fn boa_fetch_renders_network_json() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        // Minimal one-shot HTTP server returning JSON.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            // Serve a couple of connections (favicon/other probes aside, the
            // page makes one fetch); accept until the test's request is served.
            for stream in listener.incoming() {
                let mut stream = match stream {
                    Ok(s) => s,
                    Err(_) => break,
                };
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = r#"{"items":["net-a","net-b"]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
                break; // one request is enough for this test
            }
        });

        let base = format!("http://127.0.0.1:{port}/");
        let html = "<html><head><style>body{margin:0}</style></head><body>\
            <ul id=\"list\"></ul>\
            <script>\
              fetch('data.json').then(function(r){return r.json();}).then(function(d){\
                var ul=document.getElementById('list');\
                for(var i=0;i<d.items.length;i++){var li=document.createElement('li');li.textContent=d.items[i];ul.appendChild(li);}\
              });\
            </script></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 400, height: 300 };
        let result = build_layout_scene_with_script_runtime(&base, html, &rect);

        // The fetched items were rendered into the document (the bounded-wait
        // lifecycle settled the promise before layout).
        let has_a = result.render_tree_contains("net-a");
        let has_b = result.render_tree_contains("net-b");
        let _ = server.join();
        assert!(has_a && has_b, "fetched items not rendered; diagnostics={:?}", result.diagnostics);
    }

    #[test]
    fn live_page_progressive_render_after_fetch() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            if let Some(Ok(mut stream)) = listener.incoming().next() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = r#"{"items":["late-x","late-y"]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

        let base = format!("http://127.0.0.1:{port}/");
        let html = "<html><head><style>body{margin:0}</style></head><body>\
            <ul id=\"list\"></ul>\
            <script>\
              fetch('data.json').then(function(r){return r.json();}).then(function(d){\
                var ul=document.getElementById('list');\
                for(var i=0;i<d.items.length;i++){var li=document.createElement('li');li.textContent=d.items[i];ul.appendChild(li);}\
              });\
            </script></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 400, height: 300 };

        // A waker fires when the fetch response is ready (drives the render
        // loop wake-up in the GUI).
        let woken = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let woken2 = woken.clone();
        let waker: crate::loader::FetchWaker =
            std::sync::Arc::new(move || woken2.store(true, std::sync::atomic::Ordering::SeqCst));

        // First paint happens immediately, before the fetch resolves.
        let (mut page, first_scene) = LivePage::load(&base, html, &rect, Some(waker));
        let first_has = first_scene.scene_items.iter().any(|i| matches!(i, SceneItem::Text { text, .. } if text.contains("late-")));
        assert!(!first_has, "fetched data should NOT be in the first paint");
        assert!(page.has_pending_work(), "the fetch should still be in flight");

        // Poll for completion (worker thread), then re-lay-out the SAME page.
        // pump_and_relayout returns Some only when the DOM actually changed.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut scene = first_scene;
        let mut relaid_out = false;
        while page.has_pending_work() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
            if let Some(updated) = page.pump_and_relayout(&rect) {
                scene = updated;
                relaid_out = true;
            }
        }
        let _ = server.join();

        assert!(relaid_out, "the fetch completion should have triggered a re-layout");
        let now_has_x = scene.scene_items.iter().any(|i| matches!(i, SceneItem::Text { text, .. } if text.contains("late-x")));
        let now_has_y = scene.scene_items.iter().any(|i| matches!(i, SceneItem::Text { text, .. } if text.contains("late-y")));
        assert!(now_has_x && now_has_y, "progressive re-layout did not render the fetched items");
        assert!(
            woken.load(std::sync::atomic::Ordering::SeqCst),
            "the fetch waker should have fired to wake the render loop"
        );
    }

    #[test]
    fn pump_without_dom_mutation_skips_relayout() {
        // A page with no async work and no pending mutations: pumping must
        // return None (nothing changed → no wasted re-layout).
        let html = "<html><body><p id=\"p\">hi</p></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 400, height: 300 };
        let (mut page, _scene) = LivePage::load("about:blank", html, &rect, None);
        assert!(!page.has_pending_work());
        assert!(
            page.pump_and_relayout(&rect).is_none(),
            "an idle pump should not trigger a re-layout"
        );
    }
}

#[cfg(test)]
impl ScriptLayoutResult {
    /// Whether any text node in the rendered scene contains `needle`.
    fn render_tree_contains(&self, needle: &str) -> bool {
        self.layout_scene.scene_items.iter().any(|item| match item {
            SceneItem::Text { text, .. } => text.contains(needle),
            _ => false,
        })
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
            background_gradient: None,
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
            background_gradient: None,
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
