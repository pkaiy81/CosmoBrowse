use crate::model::{
    ContentSize, FrameRect, RenderBox, RenderNode, RenderNodeKind, RenderTreeSnapshot,
    ResolvedStyle, SceneItem,
};
use crate::security::{local_storage_snapshot, replace_local_storage};
use cosmo_engine::renderer::layout::layout_object::CaretStop;
use cosmo_engine::renderer::css::cssom::CssParser;
use cosmo_engine::renderer::css::token::CssTokenizer;
use crate::forms;
use crate::loader::fetch_external_stylesheets;
use cosmo_engine::renderer::dom::api::{
    get_style_content, get_stylesheet_links,
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
use cosmo_engine::renderer::layout::layout_view::{AnimationTarget, LayoutView, TransitionTarget};
use cosmo_engine::display_item::DisplayItem;
use std::cell::RefCell;
use std::rc::Rc;

mod keyframes;
mod transitions;
use keyframes::KeyframeDriver;
use transitions::TransitionDriver;

/// One animation frame of the GUI's ~60fps clock. The script host's virtual
/// timer clock and the transition driver advance in the same steps so headless
/// runs (which drive frames as fast as they can) stay deterministic.
const FRAME_MS: u64 = 16;

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
    let cssom = resolve_cssom(dom.clone(), document_url);
    layout_dom_with_style(dom, &cssom, rect)
}

/// Fetch external `<link>` sheets + inline `<style>` and parse them into a
/// `StyleSheet`. Split out so [`LivePage`] can parse once and reuse the CSSOM
/// across pumps/reflows (a DOM mutation from script rarely changes the
/// stylesheets; the expensive re-parse is skipped).
///
/// Spec: CSS Cascading §6 — declaration order within the same origin.
/// External sheets are applied first so a later inline `<style>` wins on equal
/// specificity (approximating document order). Fetching is cached by URL.
pub(crate) fn resolve_cssom(
    dom: Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>,
    document_url: &str,
) -> cosmo_engine::renderer::css::cssom::StyleSheet {
    let links = get_stylesheet_links(dom.clone());
    let external_css = fetch_external_stylesheets(document_url, &links);
    let inline_css = get_style_content(dom.clone());
    let style = if external_css.is_empty() {
        inline_css
    } else {
        format!("{external_css}\n{inline_css}")
    };
    CssParser::new(CssTokenizer::new(style)).parse_stylesheet()
}

/// Build the layout view for `dom` against an already-parsed `cssom` at `rect`.
fn build_layout_view(
    dom: Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>,
    cssom: &cosmo_engine::renderer::css::cssom::StyleSheet,
    rect: &FrameRect,
) -> LayoutView {
    // var(--token) references are resolved per element during the cascade
    // (custom properties inherit; the document root seeds from the whole
    // stylesheet), so no global pre-substitution is needed here.
    LayoutView::new_with_viewport(dom, cssom, rect.width.max(1), rect.height.max(0))
}

/// Lay out `dom` against an already-parsed `cssom` at `rect`, producing both
/// the paint scene and the render-tree snapshot.
fn layout_dom_with_style(
    dom: Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>,
    cssom: &cosmo_engine::renderer::css::cssom::StyleSheet,
    rect: &FrameRect,
) -> (LayoutScene, RenderTreeSnapshot) {
    let layout_view = build_layout_view(dom, cssom, rect);
    let layout_scene = display_items_to_scene(layout_view.paint(), rect);
    let render_tree = render_tree_snapshot(&layout_view, rect);
    (layout_scene, render_tree)
}

/// Lay out `dom` producing **only** the paint scene (skips the render-tree
/// snapshot, which the GUI's LivePage discards — saving that whole-tree walk
/// on every progressive update / reflow).
fn layout_scene_only(
    dom: Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>,
    cssom: &cosmo_engine::renderer::css::cssom::StyleSheet,
    rect: &FrameRect,
) -> LayoutScene {
    let layout_view = build_layout_view(dom, cssom, rect);
    display_items_to_scene(layout_view.paint(), rect)
}

/// As [`layout_scene_only`], plus the transition targets the cascade computed
/// in the same pass (the driver needs both from one layout, and building the
/// view twice would double the cost).
fn layout_scene_and_targets(
    dom: Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>,
    cssom: &cosmo_engine::renderer::css::cssom::StyleSheet,
    rect: &FrameRect,
) -> (LayoutScene, Vec<TransitionTarget>, Vec<AnimationTarget>) {
    let layout_view = build_layout_view(dom, cssom, rect);
    let scene = display_items_to_scene(layout_view.paint(), rect);
    let transitions = layout_view.collect_transition_targets();
    let animations = layout_view.collect_animation_targets(cssom);
    (scene, transitions, animations)
}

/// What dispatching a click into a [`LivePage`] produced.
#[derive(Debug, Default)]
pub struct ClickOutcome {
    /// Whether the click landed on a box at all (false = no element there, and
    /// nothing was dispatched).
    pub hit: bool,
    /// A handler called `preventDefault()`: the caller must not run the default
    /// activation behaviour (following a link).
    pub default_prevented: bool,
    /// Fresh scene, when the handlers mutated the DOM (else nothing to repaint).
    pub scene: Option<LayoutScene>,
    /// `postMessage` payloads posted while handling the click — the navigation
    /// shim injected by `loader::prepare_html_for_display` posts its
    /// `cosmobrowse:navigate` request here.
    pub messages: Vec<String>,
}

/// A hash of everything `resolve_cssom` reads.
fn stylesheets_fingerprint(dom: &Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    get_style_content(dom.clone()).hash(&mut hasher);
    get_stylesheet_links(dom.clone()).hash(&mut hasher);
    hasher.finish()
}

/// Parses the stylesheets again if script has changed them: CSS-in-JS, or a sheet
/// loaded late.
fn refresh_stylesheets(
    dom: &Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>,
    document_url: &str,
    cssom: &RefCell<cosmo_engine::renderer::css::cssom::StyleSheet>,
    fingerprint: &std::cell::Cell<u64>,
) {
    let current = stylesheets_fingerprint(dom);
    if current != fingerprint.get() {
        fingerprint.set(current);
        *cssom.borrow_mut() = resolve_cssom(dom.clone(), document_url);
    }
}

/// How long script's layout queries get fresh answers however much they cost.
/// Past this, a page that keeps changing the DOM and measuring it gets answers that
/// may be a moment old, so that it can't spend all its time in layout.
const FREE_LAYOUT_QUERY_TIME: std::time::Duration = std::time::Duration::from_millis(1500);

/// Layout, for script's questions about it.
struct PageLayoutProvider {
    dom: Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>,
    document_url: String,
    cssom: Rc<RefCell<cosmo_engine::renderer::css::cssom::StyleSheet>>,
    css_fingerprint: Rc<std::cell::Cell<u64>>,
    rect: Rc<RefCell<FrameRect>>,
    current: RefCell<Option<CurrentLayout>>,
    time_in_layout: std::cell::Cell<std::time::Duration>,
}

/// The layout script's queries read, with its boxes found by node.
struct CurrentLayout {
    generation: u64,
    laid_out_at: std::time::Instant,
    took: std::time::Duration,
    _view: LayoutView,
    boxes: std::collections::HashMap<
        usize,
        Rc<RefCell<cosmo_engine::renderer::layout::layout_object::LayoutObject>>,
    >,
}

impl PageLayoutProvider {
    fn ensure_layout(&self, generation: u64) {
        if let Some(current) = self.current.borrow().as_ref() {
            if current.generation == generation {
                return;
            }
            // Keep layout to about a quarter of the time, however often it's asked.
            if self.time_in_layout.get() > FREE_LAYOUT_QUERY_TIME
                && current.laid_out_at.elapsed() < current.took * 3
            {
                return;
            }
        }
        let started = std::time::Instant::now();
        refresh_stylesheets(&self.dom, &self.document_url, &self.cssom, &self.css_fingerprint);
        let rect = self.rect.borrow().clone();
        let view = build_layout_view(self.dom.clone(), &self.cssom.borrow(), &rect);
        let mut boxes = std::collections::HashMap::new();
        fn index(
            object: Option<Rc<RefCell<cosmo_engine::renderer::layout::layout_object::LayoutObject>>>,
            boxes: &mut std::collections::HashMap<
                usize,
                Rc<RefCell<cosmo_engine::renderer::layout::layout_object::LayoutObject>>,
            >,
        ) {
            let mut current = object;
            while let Some(o) = current {
                let key = Rc::as_ptr(&o.borrow().node_ref()) as *const () as usize;
                boxes.entry(key).or_insert_with(|| o.clone());
                index(o.borrow().first_child(), boxes);
                current = o.borrow().next_sibling();
            }
        }
        index(view.root(), &mut boxes);
        self.time_in_layout.set(self.time_in_layout.get() + started.elapsed());
        *self.current.borrow_mut() = Some(CurrentLayout {
            generation,
            laid_out_at: std::time::Instant::now(),
            took: started.elapsed(),
            _view: view,
            boxes,
        });
    }

    fn with_box<T>(
        &self,
        node: &Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>,
        generation: u64,
        read: impl FnOnce(&cosmo_engine::renderer::layout::layout_object::LayoutObject) -> T,
    ) -> Option<T> {
        self.ensure_layout(generation);
        let current = self.current.borrow();
        let key = Rc::as_ptr(node) as *const () as usize;
        let object = current.as_ref()?.boxes.get(&key)?.clone();
        let object = object.borrow();
        Some(read(&object))
    }
}

impl cosmo_script::LayoutProvider for PageLayoutProvider {
    fn rect(
        &self,
        node: &Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>,
        generation: u64,
    ) -> Option<[f64; 4]> {
        self.with_box(node, generation, |object| {
            [
                object.point().x() as f64,
                object.point().y() as f64,
                object.size().width() as f64,
                object.size().height() as f64,
            ]
        })
    }

    fn computed_style(
        &self,
        node: &Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>,
        generation: u64,
    ) -> Vec<(String, String)> {
        // No box means `display: none`, or something inside it.
        self.with_box(node, generation, |object| object.computed_style_properties())
            .unwrap_or_else(|| vec![("display".to_string(), "none".to_string())])
    }

    fn viewport(&self) -> (f64, f64) {
        let rect = self.rect.borrow();
        (rect.width as f64, rect.height as f64)
    }
}

/// Marks a `<select>` as showing its list, for the painter.
const OPEN_SELECT_ATTRIBUTE: &str = "data-cosmo-open";

fn collect_options(
    node: &Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>,
    out: &mut Vec<Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>>,
) {
    let mut child = node.borrow().first_child();
    while let Some(c) = child {
        if c.borrow().element_tag_name().as_deref() == Some("option") {
            out.push(c.clone());
        }
        collect_options(&c, out);
        let next = c.borrow().next_sibling();
        child = next;
    }
}

/// A caret movement requested from the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaretMove {
    Left,
    Right,
    Up,
    Down,
    LineStart,
    LineEnd,
}

/// Where to draw the caret: frame-local document coordinates, and the field's
/// text color.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaretRect {
    pub x: i64,
    pub y: i64,
    pub height: i64,
    pub color: String,
}

/// The stop on the line nearest `point`, and on that line the one nearest
/// `point` horizontally.
fn nearest_caret_stop(stops: &[CaretStop], point: (i64, i64)) -> usize {
    let mut best = 0;
    let mut best_distance = (i64::MAX, i64::MAX);
    for (index, stop) in stops.iter().enumerate() {
        let vertical = if point.1 < stop.y {
            stop.y - point.1
        } else {
            (point.1 - (stop.y + stop.height)).max(0)
        };
        let distance = (vertical, (stop.x - point.0).abs());
        if distance < best_distance {
            best_distance = distance;
            best = index;
        }
    }
    best
}

/// The caret position `movement` leads to from `caret`. Vertical moves work on
/// the painted lines, so they cross soft wraps as well as newlines; with no
/// line to move to they go to the start or end of the value.
fn moved_caret(stops: &[CaretStop], caret: usize, movement: CaretMove) -> usize {
    let last = stops.len() - 1;
    let here = stops[caret];
    let on_line = |index: &usize| stops[*index].y == here.y;
    match movement {
        CaretMove::Left => caret.saturating_sub(1),
        CaretMove::Right => (caret + 1).min(last),
        CaretMove::LineStart => (0..=caret).rev().take_while(|i| on_line(i)).last().unwrap_or(caret),
        CaretMove::LineEnd => (caret..=last).take_while(|i| on_line(i)).last().unwrap_or(caret),
        CaretMove::Up => stops[..caret]
            .iter()
            .rev()
            .find(|stop| stop.y < here.y)
            .map(|line| {
                let line_y = line.y;
                let candidates: Vec<usize> =
                    (0..caret).filter(|i| stops[*i].y == line_y).collect();
                nearest_on_line(stops, &candidates, here.x)
            })
            .unwrap_or(0),
        CaretMove::Down => stops[caret..]
            .iter()
            .find(|stop| stop.y > here.y)
            .map(|line| {
                let line_y = line.y;
                let candidates: Vec<usize> =
                    (caret..=last).filter(|i| stops[*i].y == line_y).collect();
                nearest_on_line(stops, &candidates, here.x)
            })
            .unwrap_or(last),
    }
}

fn nearest_on_line(stops: &[CaretStop], candidates: &[usize], x: i64) -> usize {
    candidates
        .iter()
        .copied()
        .min_by_key(|i| (stops[*i].x - x).abs())
        .unwrap_or(0)
}

/// The byte offset of the `chars`-th character of `value`.
fn byte_offset(value: &str, chars: usize) -> usize {
    value
        .char_indices()
        .nth(chars)
        .map_or(value.len(), |(offset, _)| offset)
}

/// Where a field keeps its text. `<input>` uses the `value` attribute, which is
/// also what layout paints; `<textarea>` uses its child text, per HTML LS
/// §4.10.11 (its value is its text content, not an attribute).
#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldStorage {
    ValueAttribute,
    ChildText,
}

/// How `node` stores its text, or `None` if it is not a text-entry field.
/// Buttons, checkboxes and the rest take clicks, not keystrokes.
/// Spec: HTML LS §4.10.5.1 — input type keywords.
fn field_storage(
    node: &Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>,
) -> Option<FieldStorage> {
    let borrowed = node.borrow();
    let NodeKind::Element(element) = borrowed.kind() else {
        return None;
    };
    match element.tag_name() {
        "textarea" => Some(FieldStorage::ChildText),
        "input" => {
            // A missing or unknown `type` is text (the missing value default).
            let kind = element
                .get_attribute("type")
                .unwrap_or_else(|| "text".to_string())
                .to_ascii_lowercase();
            matches!(
                kind.as_str(),
                "text" | "search" | "email" | "url" | "tel" | "password" | "number"
            )
            .then_some(FieldStorage::ValueAttribute)
        }
        _ => None,
    }
}

fn is_text_input(node: &Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>) -> bool {
    field_storage(node).is_some()
}

/// The field's current value.
fn current_value(node: &Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>) -> String {
    match field_storage(node) {
        Some(FieldStorage::ValueAttribute) => match node.borrow().kind() {
            NodeKind::Element(element) => element.get_attribute("value").unwrap_or_default(),
            _ => String::new(),
        },
        Some(FieldStorage::ChildText) => {
            // From the *children*: `collect_text` also walks the node's
            // following siblings, so passing the element itself swept up
            // whatever came after it in the document — the newline text node
            // after `</textarea>` ended up in the value, and every keystroke
            // landed on its own line.
            let mut text = String::new();
            cosmo_engine::renderer::dom::api::collect_text(
                node.borrow().first_child(),
                &mut text,
            );
            text
        }
        None => String::new(),
    }
}

/// Write `value` back to the field, wherever it keeps it.
fn set_field_value(
    node: &Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>,
    value: &str,
) {
    match field_storage(node) {
        Some(FieldStorage::ValueAttribute) => {
            let mut borrowed = node.borrow_mut();
            if let NodeKind::Element(element) = borrowed.kind_mut() {
                element.set_attribute("value", value);
            }
        }
        Some(FieldStorage::ChildText) => {
            // Reuse the existing text node when there is one so the tree keeps
            // its shape; otherwise give the element a single text child.
            let existing = node.borrow().first_child();
            match existing {
                Some(child)
                    if matches!(child.borrow().kind(), NodeKind::Text(_)) =>
                {
                    *child.borrow_mut().kind_mut() = NodeKind::Text(value.to_string());
                }
                _ => {
                    let text = Rc::new(RefCell::new(
                        cosmo_engine::renderer::dom::node::Node::new(NodeKind::Text(
                            value.to_string(),
                        )),
                    ));
                    text.borrow_mut().set_parent(Rc::downgrade(node));
                    node.borrow_mut().set_first_child(Some(text));
                }
            }
        }
        None => {}
    }
}

/// Node identities of `node` and every element ancestor, nearest first — the
/// set `:hover` matches for a pointer over `node`.
fn ancestor_chain(
    node: Option<Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>>,
) -> Vec<usize> {
    let mut chain = Vec::new();
    let mut current = node;
    while let Some(n) = current {
        if matches!(n.borrow().kind(), NodeKind::Element(_)) {
            chain.push(Rc::as_ptr(&n) as *const () as usize);
        }
        let parent = n.borrow().parent().upgrade();
        current = parent;
    }
    chain
}

/// The nearest element at or above `node` — a click that lands on a text box
/// targets the element containing it, as in the DOM's event model.
fn nearest_element(
    node: Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>,
) -> Option<Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>> {
    let mut current = Some(node);
    while let Some(n) = current {
        if matches!(n.borrow().kind(), NodeKind::Element(_)) {
            return Some(n.clone());
        }
        let parent = n.borrow().parent().upgrade();
        current = parent;
    }
    None
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
    /// The `<select>` whose list is showing.
    open_select: Option<Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>>,
    /// A hash of the stylesheets `cssom` was parsed from: `<style>` and `<link>`
    /// elements script has changed mean parsing them again.
    css_fingerprint: Rc<std::cell::Cell<u64>>,
    /// The frame's rect at the last layout, which script's queries lay out at.
    layout_rect: Rc<RefCell<FrameRect>>,
    /// The parsed stylesheets, resolved once at load and reused across
    /// pumps/reflows (script DOM mutations rarely change the stylesheets, so
    /// the expensive CSS re-parse is skipped — the first incremental win).
    cssom: Rc<RefCell<cosmo_engine::renderer::css::cssom::StyleSheet>>,
    /// DOM mutation generation at the last layout; a pump that leaves it
    /// unchanged skips re-layout (no script tick touched the DOM).
    last_generation: u64,
    /// Declarative CSS `transition` state (Phase 4.4): watches the cascade's
    /// target values across layouts and interpolates the in-between frames.
    transitions: TransitionDriver,
    /// `@keyframes` playback state (Phase 4.4): plays declared timelines.
    keyframes: KeyframeDriver,
    /// The text field the user is typing into, if any. Keystrokes are routed
    /// here instead of to the browser chrome.
    focused_field: Option<Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>>,
    /// The caret, as a character position in the focused field's value. A script
    /// can shorten the value between edits, so read it through `caret_index`.
    caret: usize,
    /// The focused field's caret stops and text color, kept until the next
    /// layout because finding them takes a layout of its own.
    caret_stops: Option<(Vec<CaretStop>, String)>,
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
        // The hover and focus chains key on node addresses, which a freed
        // document can hand back to a new one — drop them before the new DOM
        // exists.
        cosmo_engine::renderer::style::values::set_hover_chain(&[]);
        cosmo_engine::renderer::style::values::set_focus_chain(&[]);
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

        // Scripts can ask about layout while they run, so the stylesheets have to
        // be there first.
        let cssom = Rc::new(RefCell::new(resolve_cssom(dom.clone(), document_url)));
        let css_fingerprint = Rc::new(std::cell::Cell::new(stylesheets_fingerprint(&dom)));
        let layout_rect = Rc::new(RefCell::new(rect.clone()));
        host.set_layout_provider(Box::new(PageLayoutProvider {
            dom: dom.clone(),
            document_url: document_url.to_string(),
            cssom: cssom.clone(),
            css_fingerprint: css_fingerprint.clone(),
            rect: layout_rect.clone(),
            current: RefCell::new(None),
            time_in_layout: std::cell::Cell::new(std::time::Duration::ZERO),
        }));

        host.run_document_scripts();
        // The document is parsed by the time we get here, so the handlers the
        // scripts just registered for it fire now.
        host.fire_dom_content_loaded();
        // Only what is due at t=0. A retained page has a frame clock
        // (`animation_frame`) to deliver delayed timers on schedule, so unlike
        // the one-shot pipeline it must not flush them up front — that would make
        // timed UI (and the transitions it triggers) snap to its end state before
        // the first paint.
        host.run_due(1000);
        host.fire_load();
        host.run_due(1000);
        replace_local_storage(document_url, &host.local_storage_entries());

        let last_generation = host.dom_generation();
        let mut page = Self {
            host,
            dom,
            document_url: document_url.to_string(),
            cssom,
            css_fingerprint,
            layout_rect,
            last_generation,
            transitions: TransitionDriver::default(),
            keyframes: KeyframeDriver::default(),
            focused_field: None,
            open_select: None,
            caret: 0,
            caret_stops: None,
        };
        // The first layout only records the transition baselines: an element's
        // initial style never animates (CSS Transitions §2).
        let scene = page.layout_and_drive(rect);
        (page, scene)
    }

    /// Whether asynchronous work (fetch/XHR) is still outstanding — i.e. another
    /// `pump_and_relayout` may yield an updated scene.
    pub fn has_pending_work(&self) -> bool {
        self.host.has_pending_fetches()
    }

    #[cfg(test)]
    fn eval_for_test(&mut self, script: &str) -> String {
        self.host.eval_to_string(script).unwrap_or_else(|e| e)
    }

    /// Whether the page has an ongoing animation the GUI should keep driving
    /// frames for: queued timers/rAF, or a CSS transition mid-flight.
    pub fn has_pending_animation(&self) -> bool {
        self.host.has_pending_timers()
            || self.transitions.is_animating()
            || self.keyframes.is_animating()
    }

    /// Advance one animation frame (~16ms): step running CSS transitions and
    /// run due timers/rAF; if either moved, re-lay-out and return the fresh
    /// scene (else None). Used by the GUI frame clock.
    pub fn animation_frame(&mut self, rect: &FrameRect) -> Option<LayoutScene> {
        self.host.run_frame(FRAME_MS, 256);
        replace_local_storage(&self.document_url, &self.host.local_storage_entries());
        let transitions_moved = self.transitions.advance(FRAME_MS as f64);
        let keyframes_moved = self.keyframes.advance(FRAME_MS as f64);
        let generation = self.host.dom_generation();
        if generation == self.last_generation && !transitions_moved && !keyframes_moved {
            return None;
        }
        self.last_generation = generation;
        Some(self.layout_and_drive(rect))
    }

    /// Re-lay-out the retained DOM at `rect` **without** running scripts or
    /// pumping async work (used on viewport resize — a reflow, not a re-run).
    pub fn relayout(&mut self, rect: &FrameRect) -> LayoutScene {
        self.layout_and_drive(rect)
    }

    /// Dispatch a press at `point` (frame-local document coordinates, i.e. the
    /// click position with scroll and the frame's own origin already removed).
    /// The deepest box containing the point is hit-tested against a fresh
    /// layout, the full `mousedown` → `mouseup` → `click` sequence is fired on
    /// the nearest enclosing *element*, and any DOM mutation the handlers made
    /// is laid out again.
    ///
    /// Only `click` is cancellable here: `preventDefault()` on it suppresses
    /// the activation behaviour (link following).
    /// Spec: UI Events §3.5 — the click sequence.
    /// https://w3c.github.io/uievents/#event-type-click
    pub fn dispatch_click(&mut self, point: (i64, i64), rect: &FrameRect) -> ClickOutcome {
        if let Some(select) = self.open_select.clone() {
            return self.click_open_select(select, point, rect);
        }
        let mut outcome = ClickOutcome::default();
        let target = {
            let view = build_layout_view(self.dom.clone(), &self.cssom.borrow(), rect);
            view.find_node_by_position(point)
                .map(|object| object.borrow().node_ref())
                .and_then(nearest_element)
        };
        let Some(target) = target else {
            return outcome;
        };
        outcome.hit = true;
        let (x, y) = (point.0 as f64, point.1 as f64);
        for event_type in ["mousedown", "mouseup"] {
            self.host
                .dispatch_mouse_event(target.clone(), event_type, x, y);
        }
        outcome.default_prevented =
            !self.host.dispatch_mouse_event(target.clone(), "click", x, y);
        let mut changed = false;
        if !outcome.default_prevented {
            changed = self.run_activation(forms::activate(&target), &mut outcome);
        }
        replace_local_storage(&self.document_url, &self.host.local_storage_entries());
        outcome.messages.extend(self.host.take_posted_messages());

        let generation = self.host.dom_generation();
        if generation != self.last_generation || changed {
            self.last_generation = generation;
            outcome.scene = Some(self.layout_and_drive(rect));
        }
        outcome
    }

    /// Carry out what a click on a control does once the page has had its say.
    /// Returns whether the document changed.
    fn run_activation(
        &mut self,
        activation: forms::Activation,
        outcome: &mut ClickOutcome,
    ) -> bool {
        match activation {
            forms::Activation::None | forms::Activation::FocusField(_) => false,
            forms::Activation::Toggled(control) => {
                self.host.dispatch_event(control.clone(), "input");
                self.host.dispatch_event(control, "change");
                true
            }
            forms::Activation::ToggleDetails(details) => {
                let open = matches!(
                    details.borrow().kind,
                    NodeKind::Element(ref e) if e.get_attribute("open").is_some()
                );
                if open {
                    forms::remove_attribute(&details, "open");
                } else {
                    forms::set_attribute(&details, "open", "");
                }
                self.host.dispatch_event(details, "toggle");
                true
            }
            forms::Activation::ToggleSelect(select) => {
                self.open_select = Some(select.clone());
                forms::set_attribute(&select, OPEN_SELECT_ATTRIBUTE, "");
                true
            }
            forms::Activation::Submit { form, submitter } => {
                self.submit_form(&form, submitter.as_ref(), outcome);
                false
            }
        }
    }

    /// Fire `submit`, and unless the page cancels it, ask to navigate.
    fn submit_form(
        &mut self,
        form: &Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>,
        submitter: Option<&Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>>,
        outcome: &mut ClickOutcome,
    ) {
        if !self.host.dispatch_event(form.clone(), "submit") {
            return;
        }
        let Some(submission) = forms::build_submission(form, submitter, &self.document_url)
        else {
            return;
        };
        outcome.messages.push(
            serde_json::json!({
                "type": "cosmobrowse:navigate",
                "href": submission.url,
                "target": submission.target,
                "postBody": submission.post_body,
            })
            .to_string(),
        );
    }

    /// Enter in a single-line field. Returns the navigation messages it caused.
    pub fn submit_focused_field(&mut self) -> Vec<String> {
        let mut outcome = ClickOutcome::default();
        let field = self.focused_field.clone();
        if let Some(forms::Activation::Submit { form, submitter }) =
            field.as_ref().and_then(forms::implicit_submission)
        {
            self.submit_form(&form, submitter.as_ref(), &mut outcome);
        }
        outcome.messages.extend(self.host.take_posted_messages());
        outcome.messages
    }

    /// A click while a `<select>`'s list is open goes to the list: an option
    /// picks it, and anywhere else just closes it.
    fn click_open_select(
        &mut self,
        select: Rc<RefCell<cosmo_engine::renderer::dom::node::Node>>,
        point: (i64, i64),
        rect: &FrameRect,
    ) -> ClickOutcome {
        let picked = {
            let view = build_layout_view(self.dom.clone(), &self.cssom.borrow(), rect);
            view.find_object_of_node(&select).and_then(|object| {
                let object = object.borrow();
                let row = object.select_popup_rows().iter().position(|(x, y, w, h)| {
                    point.0 >= *x && point.0 < x + w && point.1 >= *y && point.1 < y + h
                })?;
                object.select_options().get(row).map(|(_, node)| node.clone())
            })
        };
        forms::remove_attribute(&select, OPEN_SELECT_ATTRIBUTE);
        self.open_select = None;
        if let Some(option) = picked {
            let mut options = Vec::new();
            collect_options(&select, &mut options);
            for other in options {
                forms::remove_attribute(&other, "selected");
            }
            forms::set_attribute(&option, "selected", "");
            self.host.dispatch_event(select.clone(), "input");
            self.host.dispatch_event(select, "change");
        }
        ClickOutcome {
            hit: true,
            default_prevented: true,
            scene: Some(self.layout_and_drive(rect)),
            messages: self.host.take_posted_messages(),
        }
    }

    /// Whether this document has any `:hover` rule. Pointer motion over a page
    /// without one changes nothing, so the renderer can skip hover tracking
    /// (which costs a hit-test plus a re-style) entirely.
    pub fn uses_hover(&self) -> bool {
        self.cssom.borrow().uses_hover()
    }

    /// Point the `:hover` state at `point` (frame-local document coordinates),
    /// or clear it with `None` when the pointer leaves. Returns a fresh scene
    /// when the hovered element chain actually changed — which is also where a
    /// hover-triggered CSS transition starts, since the re-style moves the
    /// declared target.
    ///
    /// Spec: Selectors 4 §9.1 — `:hover` applies to the element under the
    /// pointer *and its ancestors*. https://www.w3.org/TR/selectors-4/#the-hover-pseudo
    pub fn set_hover_point(
        &mut self,
        point: Option<(i64, i64)>,
        rect: &FrameRect,
    ) -> Option<LayoutScene> {
        let chain = match point {
            None => Vec::new(),
            Some(point) => {
                let hovered = {
                    let view = build_layout_view(self.dom.clone(), &self.cssom.borrow(), rect);
                    view.find_node_by_position(point)
                        .map(|object| object.borrow().node_ref())
                        .and_then(nearest_element)
                };
                ancestor_chain(hovered)
            }
        };
        if !cosmo_engine::renderer::style::values::set_hover_chain(&chain) {
            return None;
        }
        Some(self.layout_and_drive(rect))
    }

    /// Focus the text field at `point` (frame-local document coordinates), or
    /// clear the focus when there is no field there. Returns whether a field is
    /// now focused, which is how the caller knows to route keystrokes here
    /// rather than to the URL bar.
    pub fn focus_field_at(&mut self, point: (i64, i64), rect: &FrameRect) -> bool {
        let hit = {
            let view = build_layout_view(self.dom.clone(), &self.cssom.borrow(), rect);
            view.find_node_by_position(point)
                .map(|object| object.borrow().node_ref())
                .and_then(nearest_element)
        };
        self.focused_field = hit.filter(is_text_input);
        self.publish_focus_chain();
        self.caret_stops = None;
        // The click also places the caret, whether or not the field already had
        // the focus.
        if let Some(stops) = self.current_caret_stops(rect) {
            self.caret = nearest_caret_stop(stops, point);
        }
        self.focused_field.is_some()
    }

    /// Tell the style engine which element is focused, so `:focus` and
    /// `:focus-within` resolve. Returns whether it changed — the caller
    /// re-lays-out when it did, since focus styling moves boxes.
    fn publish_focus_chain(&self) -> bool {
        let chain = ancestor_chain(self.focused_field.clone());
        cosmo_engine::renderer::style::values::set_focus_chain(&chain)
    }

    /// Re-lay-out if the focus change altered any `:focus` styling.
    pub fn relayout_for_focus(&mut self, rect: &FrameRect) -> Option<LayoutScene> {
        Some(self.layout_and_drive(rect))
    }

    pub fn has_focused_field(&self) -> bool {
        self.focused_field.is_some()
    }

    pub fn blur_field(&mut self) {
        self.focused_field = None;
        self.caret_stops = None;
        self.publish_focus_chain();
    }

    /// Whether this document styles anything on `:focus` / `:focus-within`.
    /// Pages that don't need no re-layout when the focus moves.
    pub fn uses_focus(&self) -> bool {
        self.cssom.borrow().uses_focus()
    }

    /// Whether the focused field holds more than one line, so Enter types a
    /// newline rather than activating anything.
    pub fn focused_field_is_multiline(&self) -> bool {
        self.focused_field
            .as_ref()
            .is_some_and(|field| field_storage(field) == Some(FieldStorage::ChildText))
    }

    /// Insert `text` at the caret in the focused field.
    pub fn insert_text(&mut self, text: &str, rect: &FrameRect) -> Option<LayoutScene> {
        self.edit_focused_value(rect, |value, caret| {
            value.insert_str(byte_offset(value, caret), text);
            caret + text.chars().count()
        })
    }

    /// Delete the character before the caret in the focused field.
    pub fn backspace(&mut self, rect: &FrameRect) -> Option<LayoutScene> {
        self.edit_focused_value(rect, |value, caret| {
            if caret == 0 {
                return 0;
            }
            value.remove(byte_offset(value, caret - 1));
            caret - 1
        })
    }

    /// Delete the character after the caret in the focused field.
    pub fn delete_forward(&mut self, rect: &FrameRect) -> Option<LayoutScene> {
        self.edit_focused_value(rect, |value, caret| {
            if caret < value.chars().count() {
                value.remove(byte_offset(value, caret));
            }
            caret
        })
    }

    /// Move the caret in the focused field. Returns whether it moved. No layout
    /// depends on it, so the scene stays as it is.
    pub fn move_caret(&mut self, movement: CaretMove, rect: &FrameRect) -> bool {
        let caret = self.caret_index();
        let Some(stops) = self.current_caret_stops(rect) else {
            return false;
        };
        let moved = moved_caret(stops, caret.min(stops.len() - 1), movement);
        self.caret = moved;
        moved != caret
    }

    /// Where to draw the caret, if a field has the focus and a box to draw it in.
    pub fn caret_rect(&mut self, rect: &FrameRect) -> Option<CaretRect> {
        let caret = self.caret_index();
        self.current_caret_stops(rect)?;
        let (stops, color) = self.caret_stops.as_ref()?;
        let stop = stops[caret.min(stops.len() - 1)];
        Some(CaretRect {
            x: stop.x,
            y: stop.y,
            height: stop.height,
            color: color.clone(),
        })
    }

    fn caret_index(&self) -> usize {
        let length = self
            .focused_field
            .as_ref()
            .map_or(0, |field| current_value(field).chars().count());
        self.caret.min(length)
    }

    fn current_caret_stops(&mut self, rect: &FrameRect) -> Option<&Vec<CaretStop>> {
        if self.caret_stops.is_none() {
            let field = self.focused_field.clone()?;
            let view = build_layout_view(self.dom.clone(), &self.cssom.borrow(), rect);
            self.caret_stops = view.field_caret_stops(&field, &current_value(&field));
        }
        self.caret_stops.as_ref().map(|(stops, _)| stops)
    }

    /// Apply `edit` to the focused field's value at the caret, fire `input`, and
    /// re-lay-out. `edit` returns the new caret position. An edit that changes
    /// nothing (backspace at the start, say) is not an edit: no event, no layout.
    ///
    /// Where the value lives depends on the element: `<input>` keeps it in the
    /// `value` attribute, `<textarea>` in its child text (HTML LS §4.10.11).
    /// Both are what layout already paints, so writing there is all the
    /// rendering needs.
    /// Spec: HTML LS §4.10.5 — the value edit steps fire `input` at the
    /// element. https://html.spec.whatwg.org/multipage/input.html
    fn edit_focused_value(
        &mut self,
        rect: &FrameRect,
        edit: impl FnOnce(&mut String, usize) -> usize,
    ) -> Option<LayoutScene> {
        let field = self.focused_field.clone()?;
        let before = current_value(&field);
        let mut value = before.clone();
        let caret = self.caret_index();
        self.caret = edit(&mut value, caret);
        if value == before {
            return None;
        }
        set_field_value(&field, &value);
        // Let the page see the edit. A handler may mutate the DOM further, so
        // the generation is re-read after dispatching.
        self.host.dispatch_event(field, "input");
        self.last_generation = self.host.dom_generation();
        Some(self.layout_and_drive(rect))
    }

    /// Lay out the retained DOM and drive declarative transitions: whenever the
    /// cascade's target for a transitioned property moved since the previous
    /// layout, a transition starts and the interpolation's *start* value is
    /// written back onto the DOM — so this frame paints the old value instead
    /// of flashing the end state. That override costs one extra layout on the
    /// frames where a transition starts or is cancelled, never on static pages.
    fn layout_and_drive(&mut self, rect: &FrameRect) -> LayoutScene {
        self.caret_stops = None;
        let resized = {
            let last = self.layout_rect.borrow();
            (last.width, last.height) != (rect.width, rect.height)
        };
        *self.layout_rect.borrow_mut() = rect.clone();
        if resized {
            self.host.fire_resize();
        }
        refresh_stylesheets(&self.dom, &self.document_url, &self.cssom, &self.css_fingerprint);
        let (scene, transitions, animations) =
            layout_scene_and_targets(self.dom.clone(), &self.cssom.borrow(), rect);
        // `@keyframes` playback first: it may write overrides that the
        // transition driver should then see as the current displayed value.
        let animated = self.keyframes.sync(&animations);
        if self.transitions.sync_targets(&transitions) | animated {
            layout_scene_only(self.dom.clone(), &self.cssom.borrow(), rect)
        } else {
            scene
        }
    }

    /// Drain any settled async work (fetch/XHR completions, timers) so their
    /// `.then`/handlers run. If that mutated the DOM (mutation generation
    /// changed), re-lay-out the retained DOM at `rect` and return the fresh
    /// scene; otherwise return `None` (nothing to repaint). Does not re-parse
    /// the HTML.
    pub fn pump_and_relayout(&mut self, rect: &FrameRect) -> Option<LayoutScene> {
        self.host.run_due(1000);
        replace_local_storage(&self.document_url, &self.host.local_storage_entries());
        let generation = self.host.dom_generation();
        if generation == self.last_generation {
            return None;
        }
        self.last_generation = generation;
        let scene = self.layout_and_drive(rect);
        self.assert_matches_full(&scene, rect);
        Some(scene)
    }

    /// Safety net (plan 4.1): when `COSMO_LAYOUT_ASSERT=1`, verify that the
    /// incrementally-produced scene (reused CSSOM / future partial relayout)
    /// is identical to a full from-scratch layout. Panics on divergence so
    /// bugs surface in CI/tests rather than as silent mis-renders. No-op in
    /// normal runs.
    fn assert_matches_full(&self, scene: &LayoutScene, rect: &FrameRect) {
        if std::env::var("COSMO_LAYOUT_ASSERT").as_deref() != Ok("1") {
            return;
        }
        let (full, _tree) = layout_dom(self.dom.clone(), &self.document_url, rect);
        assert!(
            full.scene_items == scene.scene_items,
            "incremental layout diverged from full layout (COSMO_LAYOUT_ASSERT): \
             {} incremental vs {} full scene items",
            scene.scene_items.len(),
            full.scene_items.len()
        );
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
    let mut diagnostics = Vec::new();
    let mut host = cosmo_script::ScriptHost::new();
    host.set_location(document_url);
    host.set_local_storage_entries(local_storage_snapshot(document_url));
    host.set_fetch_engine(crate::loader::make_fetch_engine(document_url));
    host.set_document(dom);

    let ran = {
        host.run_document_scripts();
        // The document is parsed by now, so its DOMContentLoaded handlers run.
        host.fire_dom_content_loaded();
        // Drain microtasks + due one-shot timers as at initial load; each
        // interval fires at most once (no spinning at first paint). Bounded to
        // cap runaway setTimeout(0) chains.
        host.run_initial_load(1000);
        host.fire_load();
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
                let border_color = if border_width > 0 {
                    // With no color of its own, a border takes the text's.
                    style
                        .border_color()
                        .unwrap_or_else(|| style.color_or_black())
                        .code()
                        .to_string()
                } else {
                    String::new()
                };
                scene_items.push(SceneItem::Rect {
                    x,
                    y,
                    width: lw,
                    height: lh,
                    background_color: style.used_background_color().code().to_string(),
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
                    opacity: style.used_opacity(),
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
                    border_radius: scaled_len(
                        ctx,
                        style.border_radius_for(layout_size.width(), layout_size.height()),
                    ),
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
                    color: style.used_color().code().to_string(),
                    font_px: font_px_scaled,
                    font_family: style.font_family(),
                    underline: style.text_decoration() == TextDecoration::Underline,
                    bold,
                    opacity: style.used_opacity(),
                    href,
                    target,
                    // Final paint-order key from the engine's stacking pass
                    // (root canvas −2M, normal flow 0, contexts ±1M+z).
                    z_index: style.paint_z(),
                    clip_rect: clip_rect.map(|c| (c.x + rect.x, c.y + rect.y, c.width, c.height)),
                    detail: cosmo_engine::paint_commands::TextDetail::from_style(&style),
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
                    opacity: style.used_opacity(),
                    href,
                    target,
                    // Final paint-order key from the engine's stacking pass
                    // (root canvas −2M, normal flow 0, contexts ±1M+z).
                    z_index: style.paint_z(),
                    clip_rect: clip_rect.map(|c| (c.x + rect.x, c.y + rect.y, c.width, c.height)),
                    fit: style.object_fit(),
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
        NodeKind::Comment(value) => ("#comment".to_string(), Some(value)),
        NodeKind::Doctype { name, .. } => ("#doctype".to_string(), Some(name)),
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
            color: style.used_color().code().to_string(),
            background_color: style.used_background_color().code().to_string(),
            font_px: style.font_size().px(),
            font_family: style.font_family(),
            opacity: style.used_opacity(),
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
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (port, server) = spawn_fixture_server(
            r#"{"items":["net-a","net-b"]}"#,
            "application/json",
            stop.clone(),
        );

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
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = server.join();
        assert!(has_a && has_b, "fetched items not rendered; diagnostics={:?}", result.diagnostics);
    }

    /// Spawn a fixture HTTP server that answers `body` to **every** connection
    /// until `stop` is set, and return its port plus the join handle.
    ///
    /// Serving exactly once is not enough: the shared `reqwest` client pools
    /// connections and may open more than one, and a request that arrives after
    /// the listener is gone waits out the 10s connect + 20s request timeouts —
    /// which is what the 30s failures in this module were.
    /// A server for a whole small site: path, content type, extra headers, body.
    /// Also records each request's `Cookie` header, in the `x-seen-cookie` of the
    /// response to `/echo-cookie`.
    fn spawn_site_server(
        routes: Vec<(&'static str, &'static str, Vec<&'static str>, &'static str)>,
        delay: std::time::Duration,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> (u16, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let handle = std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).ok();
                        let mut buf = [0u8; 4096];
                        let read = stream.read(&mut buf).unwrap_or(0);
                        let request = String::from_utf8_lossy(&buf[..read]).to_string();
                        let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                        std::thread::sleep(delay);
                        let cookie = request
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .starts_with("cookie:")
                                    .then(|| l[7..].trim().to_string())
                            })
                            .unwrap_or_default();
                        let response = if path == "/echo-cookie" {
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\
                                 Content-Length: {}\r\nConnection: close\r\n\r\n{cookie}",
                                cookie.len(),
                            )
                        } else if let Some((_, ty, headers, body)) =
                            routes.iter().find(|(p, ..)| *p == path)
                        {
                            let extra: String =
                                headers.iter().map(|h| format!("{h}\r\n")).collect();
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Type: {ty}\r\n{extra}\
                                 Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len(),
                            )
                        } else {
                            "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\
                             Connection: close\r\n\r\n"
                                .to_string()
                        };
                        let _ = stream.write_all(response.as_bytes());
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });
        (port, handle)
    }

    /// Loads `html` as `path` on a site and returns the text it put in `#out`,
    /// once its fetches and timers are through.
    fn run_page_on_site(
        routes: Vec<(&'static str, &'static str, Vec<&'static str>, &'static str)>,
        html: &str,
    ) -> String {
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (port, server) = spawn_site_server(
            routes,
            std::time::Duration::from_millis(0),
            stop.clone(),
        );
        let base = format!("http://127.0.0.1:{port}/page.html");
        let rect = FrameRect { x: 0, y: 0, width: 800, height: 600 };
        let (mut page, _) = LivePage::load(&base, html, &rect, None);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            let _ = page.pump_and_relayout(&rect);
            let _ = page.animation_frame(&rect);
            if !page.has_pending_work() && !page.has_pending_animation() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let _ = page.pump_and_relayout(&rect);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = server.join();
        page.eval_for_test("document.getElementById('out').textContent")
    }

    const JS: &str = "text/javascript";

    #[test]
    fn external_scripts_run_in_document_order_around_inline_ones() {
        let out = run_page_on_site(
            vec![
                ("/a.js", JS, vec![], "window.log=['a'];"),
                ("/b.js", JS, vec![], "log.push('b:'+!!document.getElementById('out'));"),
                ("/d.js", JS, vec![], "log.push('defer:'+!!document.getElementById('tail'));"),
            ],
            "<html><body><p id=out></p>\
             <script defer src=d.js></script>\
             <script src=a.js></script>\
             <script>log.push('inline');</script>\
             <script src=b.js></script>\
             <p id=tail></p>\
             <script>log.push('last');\
               document.addEventListener('DOMContentLoaded',function(){\
                 log.push('dcl');document.getElementById('out').textContent=log.join(' ');});\
             </script></body></html>",
        );
        assert_eq!(out, "a inline b:true last defer:true dcl");
    }

    #[test]
    fn a_failing_script_does_not_stop_the_next_one() {
        let out = run_page_on_site(
            vec![("/ok.js", JS, vec![], "window.r.push('ok');")],
            "<html><body><p id=out></p>\
             <script>window.r=[];</script>\
             <script>throw new Error('boom');</script>\
             <script>syntax error here(</script>\
             <script src=missing.js onerror=\"r.push('onerror')\"></script>\
             <script src=ok.js onload=\"r.push('onload')\"></script>\
             <script>document.getElementById('out').textContent=r.join(' ');</script>\
             </body></html>",
        );
        assert_eq!(out, "onerror ok onload");
    }

    #[test]
    fn module_scripts_import_their_dependencies() {
        let out = run_page_on_site(
            vec![
                ("/m.js", JS, vec![], "import {n} from './dep.js';\
                    import d from '/sub/def.js';\
                    document.getElementById('out').textContent=n+d+(typeof document.currentScript)\
                      +new URL(import.meta.url).pathname;"),
                ("/dep.js", JS, vec![], "export const n = 'dep';"),
                ("/sub/def.js", JS, vec![], "export default 'def';"),
            ],
            "<html><body><p id=out></p><script type=module src=m.js></script></body></html>",
        );
        assert_eq!(out, "depdefobject/m.js");
    }

    #[test]
    fn import_maps_resolve_bare_names_and_prefixes() {
        let out = run_page_on_site(
            vec![
                ("/m.js", JS, vec![], "import a from 'lib';import b from 'pkg/deep/b.js';\
                    import c from './rel.js';\
                    document.getElementById('out').textContent=a+b+c;"),
                ("/vendor/lib.v2.js", JS, vec![], "export default 'lib';"),
                ("/vendor/pkg/deep/b.js", JS, vec![], "export default 'b';"),
                ("/mapped-rel.js", JS, vec![], "export default 'mapped';"),
            ],
            "<html><head><script type=importmap>{\"imports\":{\
               \"lib\":\"/vendor/lib.v2.js\",\
               \"pkg/\":\"/vendor/pkg/\",\
               \"./rel.js\":\"/mapped-rel.js\"}}</script></head>\
             <body><p id=out></p><script type=module src=m.js></script></body></html>",
        );
        assert_eq!(out, "libbmapped");
    }

    #[test]
    fn clearing_an_interval_from_its_own_callback_stops_it() {
        let out = run_page_on_site(
            vec![],
            "<html><body><p id=out></p><script>\
             var n=0,id=setInterval(function(){n++;if(n===3)clearInterval(id);\
               document.getElementById('out').textContent=String(n);},1);\
             </script></body></html>",
        );
        assert_eq!(out, "3");
    }

    #[test]
    fn character_data_reads_and_writes_in_place() {
        let out = run_page_on_site(
            vec![],
            "<html><body><p id=out></p><div id=d><!--note-->x</div><script>\
             var d=document.getElementById('d'),c=d.firstChild,t=d.lastChild;\
             var seen=[c.nodeType,c.data,t.nodeValue];\
             c.data='changed';t.nodeValue='y';d.lastChild.data+='z';\
             seen.push(d.innerHTML,d.textContent);\
             d.textContent='';seen.push(d.childNodes.length,d.firstChild);\
             d.textContent='a';d.append('b');seen.push(d.childNodes.length,d.lastChild.parentNode===d);\
             document.getElementById('out').textContent=seen.join('|');\
             </script></body></html>",
        );
        assert_eq!(out, "8|note|x|<!--changed-->yz|yz|0||2|true");
    }

    #[test]
    fn interface_objects_answer_instanceof() {
        let out = run_page_on_site(
            vec![],
            "<html><body><p id=out></p><div id=d><i></i>t<!--c--></div><svg id=s class=k></svg><script>\
             var d=document.getElementById('d'),s=document.getElementById('s');\
             var w=document.createTreeWalker(d,NodeFilter.SHOW_ELEMENT|NodeFilter.SHOW_TEXT),seen=[];\
             while(w.nextNode())seen.push(w.currentNode.nodeType);\
             var c=document.createTreeWalker(d,NodeFilter.SHOW_TEXT,function(){return NodeFilter.FILTER_REJECT;});\
             var r=[d.childNodes instanceof NodeList,d.classList instanceof DOMTokenList,\
               s instanceof SVGElement,s.className instanceof SVGAnimatedString,s.className.baseVal,\
               seen.join(''),c.nextNode(),document.location===location,typeof ReadableStream,\
               new DOMMatrix().translate(2,3).e,navigator.userAgent.indexOf('CosmoBrowse')>0];\
             document.getElementById('out').textContent=r.join('|');\
             </script></body></html>",
        );
        assert_eq!(out, "true|true|true|true|k|13||true|function|2|true");
    }

    #[test]
    fn a_tree_walker_follows_the_spec() {
        let out = run_page_on_site(
            vec![],
            "<html><body><p id=out></p><div id=d><a><i>1</i>2</a><!--c--><b>3</b></div><template id=t><u>x</u>y</template><script>\
             var d=document.getElementById('d'),r=[];\
             var names=function(w,step){var o=[],n;while((n=w[step]()))o.push(n.nodeName);return o.join(',');};\
             r.push(names(document.createTreeWalker(d,NodeFilter.SHOW_ALL),'nextNode'));\
             var skip=document.createTreeWalker(d,NodeFilter.SHOW_ELEMENT,function(n){return n.nodeName==='A'?NodeFilter.FILTER_SKIP:NodeFilter.FILTER_ACCEPT;});\
             r.push(names(skip,'nextNode'));\
             var reject=document.createTreeWalker(d,NodeFilter.SHOW_ELEMENT,function(n){return n.nodeName==='A'?NodeFilter.FILTER_REJECT:NodeFilter.FILTER_ACCEPT;});\
             r.push(names(reject,'nextNode'));\
             var back=document.createTreeWalker(d,NodeFilter.SHOW_ELEMENT);back.currentNode=d.lastChild;\
             r.push(names(back,'previousNode'));\
             var outside=document.createTreeWalker(document,129);\
             outside.currentNode=document.getElementById('t').content;\
             r.push(names(outside,'nextNode'));\
             var family=document.createTreeWalker(d,NodeFilter.SHOW_ELEMENT);\
             r.push(family.firstChild().nodeName,family.nextSibling().nodeName,family.parentNode().nodeName);\
             var it=document.createNodeIterator(d,NodeFilter.SHOW_TEXT);r.push(it.nextNode().data,it.nextNode().data,it.previousNode().data);\
             document.getElementById('out').textContent=r.join(' ');\
             </script></body></html>",
        );
        assert_eq!(
            out,
            "A,I,#text,#text,#comment,B,#text I,B B I,A,DIV U A B DIV 1 2 1"
        );
    }

    #[test]
    fn streams_deliver_what_is_enqueued() {
        let out = run_page_on_site(
            vec![],
            "<html><body><p id=out></p><script>\
             var rs=new ReadableStream({start:function(c){c.enqueue('a');c.enqueue('b');c.close();}});\
             var r=rs.getReader(),got=[];\
             (function next(){return r.read().then(function(x){if(x.done){\
               document.getElementById('out').textContent=got.join('');return;}\
               got.push(x.value);return next();});})();\
             </script></body></html>",
        );
        assert_eq!(out, "ab");
    }

    #[test]
    fn the_legacy_parts_of_javascript_that_the_web_relies_on_work() {
        let out = run_page_on_site(
            vec![],
            "<html><body><p id=out></p><script>\
             var o={};o.__defineGetter__('g',function(){return 'g';});\
             document.getElementById('out').textContent=['abcdef'.substr(2,2),escape('a b'),o.g].join('|');\
             </script></body></html>",
        );
        assert_eq!(out, "cd|a%20b|g");
    }

    #[test]
    fn a_shadow_tree_is_kept_apart_from_the_document() {
        let out = run_page_on_site(
            vec![],
            "<html><body><p id=out></p><div id=h><span id=light>light</span></div><script>\
             var h=document.getElementById('h'),r=h.attachShadow({mode:'open'});\
             r.innerHTML='<b id=inner class=x>shadow</b><slot></slot>';\
             var inner=r.getElementById('inner'),o=[\
               document.getElementById('inner')===null,document.querySelector('.x')===null,\
               h.children.length,h.textContent,h.shadowRoot===r,r.nodeType,r.parentNode,r.host===h,\
               inner.getRootNode()===r,inner.isConnected,inner.getRootNode({composed:true})===document,\
               inner.closest('body'),r.querySelector('slot').assignedNodes().length,\
               h.querySelector('#light').assignedSlot===r.querySelector('slot')];\
             var c=document.createElement('div').attachShadow({mode:'closed'});\
             o.push(c.host.shadowRoot);\
             document.getElementById('out').textContent=o.join('|');\
             </script></body></html>",
        );
        assert_eq!(
            out,
            "true|true|1|light|true|11||true|true|true|true||1|true|"
        );
    }

    #[test]
    fn markup_can_declare_a_shadow_tree() {
        let out = run_page_on_site(
            vec![],
            "<html><body><p id=out></p>\
             <div id=open><template shadowrootmode=open><b id=in>shadow</b><slot></slot></template>light</div>\
             <div id=closed><template shadowrootmode=closed><b>hidden</b></template></div>\
             <div id=plain><template><b>inert</b></template></div><script>\
             var o=document.getElementById('open'),c=document.getElementById('closed'),p=document.getElementById('plain');\
             var r=o.shadowRoot,seen=[r.mode,r.host===o,r.getElementById('in').textContent,\
               o.querySelector('template'),o.textContent,c.shadowRoot,c.children.length,\
               p.shadowRoot,p.querySelector('template').content.firstChild.textContent];\
             var again=o.attachShadow({mode:'open'});seen.push(again===r,r.childNodes.length);\
             var threw='no';try{o.attachShadow({mode:'open'});}catch(e){threw=e.name;}seen.push(threw);\
             document.getElementById('out').textContent=seen.join('|');\
             </script></body></html>",
        );
        assert_eq!(out, "open|true|shadow||light||0||inert|true|0|NotSupportedError");
    }

    #[test]
    fn events_are_retargeted_at_the_edge_of_a_shadow_tree() {
        let out = run_page_on_site(
            vec![],
            "<html><body><p id=out></p><div id=h></div><script>\
             var h=document.getElementById('h'),r=h.attachShadow({mode:'open'});\
             r.innerHTML='<i id=t></i>';var t=r.querySelector('#t'),log=[];\
             var note=function(where){return function(e){log.push(where+':'+(e.target.id||e.target.nodeName));};};\
             t.addEventListener('x',note('in'));r.addEventListener('x',note('root'));\
             h.addEventListener('x',note('host'));document.addEventListener('x',note('doc'));\
             t.dispatchEvent(new Event('x',{bubbles:true}));log.push('/');\
             t.dispatchEvent(new Event('x',{bubbles:true,composed:true}));\
             document.getElementById('out').textContent=log.join(' ');\
             </script></body></html>",
        );
        assert_eq!(out, "in:t root:t / in:t root:t host:h doc:h");
    }

    #[test]
    fn custom_elements_hear_about_their_lifecycle() {
        let out = run_page_on_site(
            vec![],
            "<html><body><p id=out></p><x-a id=early></x-a><script>\
             var log=[];\
             class A extends HTMLElement{\
               static get observedAttributes(){return ['k'];}\
               connectedCallback(){log.push('in:'+this.id);}\
               disconnectedCallback(){log.push('out:'+this.id);}\
               attributeChangedCallback(n,o,v){log.push(n+':'+o+'>'+v);}\
             }\
             customElements.define('x-a',A);\
             var late=document.createElement('x-a');late.id='late';\
             log.push(late instanceof A);\
             late.setAttribute('k','1');late.setAttribute('other','1');\
             document.body.appendChild(late);document.body.appendChild(late);\
             late.remove();\
             var holder=document.createElement('div');document.body.appendChild(holder);\
             holder.innerHTML='<x-a id=parsed></x-a>';\
             var r=document.createElement('div');document.body.appendChild(r);\
             r.attachShadow({mode:'open'}).innerHTML='<x-a id=shadowed></x-a>';\
             holder.innerHTML='';\
             document.getElementById('out').textContent=log.join(' ');\
             </script></body></html>",
        );
        assert_eq!(
            out,
            "in:early true k:null>1 in:late out:late in:parsed in:shadowed out:parsed"
        );
    }

    #[test]
    fn a_stylesheet_object_reaches_layout() {
        let html = "<html><head><style id=s></style></head><body style=margin:0>\
            <div id=a>a</div><div id=b>b</div><div id=h></div><script>\
            document.getElementById('s').sheet.insertRule('#a{height:31px}',0);\
            var sheet=new CSSStyleSheet();sheet.replaceSync('#b{height:32px}');\
            document.adoptedStyleSheets=[sheet];\
            var r=document.getElementById('h').attachShadow({mode:'open'});\
            r.innerHTML='<div id=in>x</div>';\
            var inner=new CSSStyleSheet();inner.replaceSync(':host{display:block;height:33px}#in{height:34px}');\
            r.adoptedStyleSheets=[inner];\
            </script></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 300, height: 300 };
        let (mut page, _) = LivePage::load("about:blank", html, &rect, None);
        let height = |page: &mut LivePage, script: &str| {
            page.eval_for_test(&format!("{script}.getBoundingClientRect().height"))
        };
        assert_eq!(height(&mut page, "document.getElementById('a')"), "31");
        assert_eq!(height(&mut page, "document.getElementById('b')"), "32");
        assert_eq!(height(&mut page, "document.getElementById('h')"), "33");
        assert_eq!(height(&mut page, "document.getElementById('h').shadowRoot.getElementById('in')"), "34");
        page.eval_for_test("document.getElementById('s').sheet.deleteRule(0)");
        assert_eq!(page.eval_for_test("document.getElementById('s').sheet.cssRules.length"), "0");
    }

    #[test]
    fn scripts_inserted_by_script_load() {
        let out = run_page_on_site(
            vec![("/late.js", JS, vec![], "window.loaded='late';")],
            "<html><body><p id=out></p><script>\
             var s=document.createElement('script');s.src='late.js';\
             s.onload=function(){document.getElementById('out').textContent=window.loaded;};\
             document.head.appendChild(s);\
             </script></body></html>",
        );
        assert_eq!(out, "late");
    }

    #[test]
    fn the_dom_has_its_class_hierarchy() {
        let out = run_page_on_site(
            vec![],
            "<html><body><p id=out></p><div id=d class='a b'><span>x</span>t<input id=i value=v></div><script>\
             var d=document.getElementById('d'),r=[];\
             r.push(d instanceof HTMLElement && d instanceof Element && d instanceof Node && d instanceof EventTarget);\
             r.push(document.getElementById('i') instanceof HTMLInputElement);\
             r.push(document.getElementById('i').value);\
             r.push(d.classList.contains('b'));\
             r.push(d.firstElementChild.tagName);\
             r.push(d.children.length);\
             r.push(document.querySelector('#d span').textContent);\
             r.push(d.childNodes[1].nodeType);\
             d.append('z');r.push(d.lastChild.data);\
             var f=document.createDocumentFragment();f.appendChild(document.createElement('b'));f.appendChild(document.createElement('i'));\
             d.appendChild(f);r.push(d.querySelectorAll('b,i').length);\
             r.push(d.cloneNode(true).children.length);\
             r.push(d.querySelector('p'),d.querySelector('div'));\
             d.remove();r.push(document.getElementById('d')===null);\
             document.getElementById('out').textContent=r.join(',');\
             </script></body></html>",
        );
        assert_eq!(out, "true,true,v,true,SPAN,2,x,3,z,2,4,,,true");
    }

    #[test]
    fn moving_a_node_to_where_it_already_is_leaves_the_tree_whole() {
        let out = run_page_on_site(
            vec![],
            "<html><body><p id=out></p><div id=d><i id=a>a</i><i id=b>b</i></div><script>\
             var d=document.getElementById('d'),a=document.getElementById('a'),b=document.getElementById('b');\
             d.prepend(a);d.insertBefore(b,b);d.insertBefore(a,a);\
             var inner='';try{a.appendChild(d);}catch(e){inner=e.message.split(':')[0];}\
             var wrong='';try{d.insertBefore(document.createElement('u'),document.body);}catch(e){wrong=e.message.split(':')[0];}\
             document.getElementById('out').textContent=d.textContent+'|'+inner+'|'+wrong;\
             </script></body></html>",
        );
        assert_eq!(out, "ab|HierarchyRequestError|NotFoundError");
    }

    #[test]
    fn a_collection_with_live_weak_map_values_finishes() {
        // Boa before 0.22 never finished marking a cycle reachable from a WeakMap
        // value, and ran out of memory trying.
        let out = run_page_on_site(
            vec![],
            "<html><body><p id=out></p><script>\
             var key={},map=new WeakMap(),cycle={};cycle.self=cycle;map.set(key,cycle);\
             var keep=[];for(var i=0;i<200000;i++){keep.push({i:i});}\
             document.getElementById('out').textContent=map.get(key)===cycle?'alive':'lost';\
             </script></body></html>",
        );
        assert_eq!(out, "alive");
    }

    #[test]
    fn script_can_measure_layout_and_read_computed_style() {
        let out = run_page_on_site(
            vec![],
            "<html><head><style>body{margin:0}\
             #b{width:120px;height:30px;margin-left:10px;color:#ff0000;--brand: blue}\
             #n{display:none}</style></head><body><div id=b></div><div id=n></div><p id=out></p><script>\
             var b=document.getElementById('b'),r=b.getBoundingClientRect(),c=getComputedStyle(b);\
             var o=[r.left,r.top,r.width,r.height,b.offsetWidth,c.color,c.display,c.width,\
               c.getPropertyValue('--brand'),getComputedStyle(document.getElementById('n')).display,innerWidth];\
             document.getElementById('out').textContent=o.join('|');\
             </script></body></html>",
        );
        assert_eq!(out, "10|0|120|30|120|rgb(255, 0, 0)|block|120px|blue|none|800");
    }

    #[test]
    fn layout_queries_see_changes_made_by_script() {
        let out = run_page_on_site(
            vec![],
            "<html><head><style>body{margin:0}</style></head><body><div id=b style='width:50px'></div>\
             <p id=out></p><script>\
             var b=document.getElementById('b');var w1=b.getBoundingClientRect().width;\
             b.style.width='70px';var w2=b.getBoundingClientRect().width;\
             document.getElementById('out').textContent=w1+' '+w2;\
             </script></body></html>",
        );
        assert_eq!(out, "50 70");
    }

    #[test]
    fn a_style_element_added_by_script_restyles_the_page() {
        let html = "<html><head><style>body{margin:0}#b{height:10px}</style></head><body>\
            <div id=b>styled</div><script>\
            setTimeout(function(){var s=document.createElement('style');\
              s.textContent='#b{height:77px;background-color:#00ff00}';\
              document.head.appendChild(s);},0);\
            </script></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 300, height: 300 };
        let (mut page, _) = LivePage::load("about:blank", html, &rect, None);
        let _ = page.animation_frame(&rect);
        let height = page
            .eval_for_test("document.getElementById('b').getBoundingClientRect().height");
        assert_eq!(height, "77");
    }

    #[test]
    fn cookies_follow_http_only_and_reach_same_origin_requests() {
        let out = run_page_on_site(
            vec![(
                "/set",
                "text/plain",
                vec!["Set-Cookie: secret=s3; HttpOnly", "Set-Cookie: plain=p1"],
                "ok",
            )],
            "<html><body><p id=out></p><script>\
             fetch('/set').then(function(){\
               var seen=document.cookie;\
               document.cookie='mine=m1';\
               document.cookie='secret=stolen';\
               return fetch('/echo-cookie').then(function(r){return r.text();}).then(function(sent){\
                 document.getElementById('out').textContent=[seen,document.cookie,sent].join('#');\
               });\
             });\
             </script></body></html>",
        );
        let parts: Vec<&str> = out.split('#').collect();
        assert_eq!(parts.len(), 3, "{out}");
        assert_eq!(parts[0], "plain=p1");
        assert!(parts[1].contains("mine=m1") && !parts[1].contains("secret"), "{out}");
        assert!(parts[2].contains("secret=s3"), "{out}");
    }

    fn spawn_fixture_server(
        body: &'static str,
        content_type: &'static str,
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> (u16, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let handle = std::thread::spawn(move || {
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).ok();
                        let mut buf = [0u8; 1024];
                        let _ = stream.read(&mut buf);
                        // Slow enough that the response can't beat the page's load,
                        // since these tests are about what happens after it.
                        std::thread::sleep(std::time::Duration::from_millis(300));
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n\
                             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len(),
                        );
                        let _ = stream.write_all(resp.as_bytes());
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        (port, handle)
    }

    /// Serializes the tests that pump a `LivePage`. `COSMO_LAYOUT_ASSERT` is
    /// process-wide, so the test that switches it on changes what every
    /// concurrently-running pump does — which showed up as either fetch test
    /// failing intermittently depending on which one happened to overlap.
    static LAYOUT_ASSERT_ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_layout_assert_env() -> std::sync::MutexGuard<'static, ()> {
        LAYOUT_ASSERT_ENV
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn live_page_progressive_render_after_fetch() {
        let _env = lock_layout_assert_env();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        // Slow enough that the response can't beat the first paint.
        let (port, server) = spawn_site_server(
            vec![(
                "/data.json",
                "application/json",
                vec![],
                r#"{"items":["late-x","late-y"]}"#,
            )],
            std::time::Duration::from_millis(500),
            stop.clone(),
        );

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
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut scene = first_scene;
        let mut relaid_out = false;
        // Wait for the outcome, not for `has_pending_work()` to go false: the
        // worker thread clears that the instant it hands the response over, so
        // polling on it can exit the loop before the `.then` chain has run.
        while !relaid_out && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
            if let Some(updated) = page.pump_and_relayout(&rect) {
                scene = updated;
                relaid_out = true;
            }
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
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
    fn incremental_matches_full_under_assert() {
        let _env = lock_layout_assert_env();
        // A fetch completes after load and its handler mutates the DOM; under
        // COSMO_LAYOUT_ASSERT the reused-CSSOM incremental pump must produce
        // scene items byte-identical to a full from-scratch layout.
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (port, server) = spawn_fixture_server(
            r#"{"items":["row0","row1","row2"]}"#,
            "application/json",
            stop.clone(),
        );

        let base = format!("http://127.0.0.1:{port}/");
        let html = "<html><head><style>body{margin:0}li{color:#123456}</style></head><body>\
            <ul id=\"l\"></ul>\
            <script>\
              fetch('d.json').then(function(r){return r.json();}).then(function(d){\
                var ul=document.getElementById('l');\
                for(var i=0;i<d.items.length;i++){var li=document.createElement('li');li.textContent=d.items[i];ul.appendChild(li);}\
              });\
            </script></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 400, height: 300 };
        let (mut page, _first) = LivePage::load(&base, html, &rect, None);

        std::env::set_var("COSMO_LAYOUT_ASSERT", "1");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut scene = None;
        // See the note in live_page_progressive_render_after_fetch: wait for
        // the re-layout, not for the pending-work flag.
        while scene.is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
            if let Some(s) = page.pump_and_relayout(&rect) {
                scene = Some(s);
            }
        }
        std::env::remove_var("COSMO_LAYOUT_ASSERT");
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = server.join();

        let scene = scene.expect("fetch mutation should trigger a re-layout (asserted == full)");
        assert!(scene.scene_items.iter().any(|i| matches!(i, SceneItem::Text { text, .. } if text.contains("row2"))));
    }

    /// Opacity of the scene rect belonging to element `#id`.
    fn scene_opacity(scene: &LayoutScene, id: &str) -> Option<f64> {
        scene.scene_items.iter().find_map(|item| match item {
            SceneItem::Rect { anchor_id, opacity, .. } if anchor_id.as_deref() == Some(id) => {
                Some(*opacity)
            }
            _ => None,
        })
    }

    #[test]
    fn css_transition_interpolates_opacity_after_a_class_change() {
        // A declarative `transition: opacity` with the target flipped by script
        // (Phase 4.4): the frames after the class change must walk 1 -> 0
        // instead of snapping, and the animation must end exactly on target.
        let html = "<html><head><style>\
            body{margin:0}\
            #box{width:100px;height:100px;background:#ff0000;opacity:1;transition:opacity 100ms linear}\
            #box.faded{opacity:0}\
            </style></head><body><div id=\"box\"></div></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 200, height: 200 };
        let (mut page, first) = LivePage::load("about:blank", html, &rect, None);
        assert_eq!(scene_opacity(&first, "box"), Some(1.0), "starts opaque");

        // Script flips the target after the first paint — what a click handler
        // or a post-load timer does in the GUI.
        let _ = page
            .host
            .eval_to_string("document.getElementById('box').className='faded'");

        let mut values = Vec::new();
        // The transition runs 100ms (~7 frames of 16ms); 40 is a generous bound.
        for _ in 0..40 {
            if let Some(scene) = page.animation_frame(&rect) {
                if let Some(opacity) = scene_opacity(&scene, "box") {
                    values.push(opacity);
                }
            }
            if !page.has_pending_animation() {
                break;
            }
        }

        assert!(
            values.windows(2).all(|w| w[1] <= w[0] + 1e-9),
            "opacity must decrease monotonically, got {values:?}"
        );
        assert!(
            values.iter().any(|v| *v > 0.01 && *v < 0.99),
            "expected interpolated frames between the endpoints, got {values:?}"
        );
        assert_eq!(
            values.last().copied(),
            Some(0.0),
            "the transition must settle on the target, got {values:?}"
        );
        assert!(!page.has_pending_animation(), "the frame clock should idle again");
    }

    /// Background color of the scene rect belonging to element `#id`.
    fn scene_background(scene: &LayoutScene, id: &str) -> Option<String> {
        scene.scene_items.iter().find_map(|item| match item {
            SceneItem::Rect { anchor_id, background_color, .. }
                if anchor_id.as_deref() == Some(id) =>
            {
                Some(background_color.clone())
            }
            _ => None,
        })
    }

    #[test]
    fn css_transition_interpolates_background_color() {
        // The driver is property-generic: a `background-color` transition walks
        // the channels instead of snapping (black -> white through the greys).
        let html = "<html><head><style>\
            body{margin:0}\
            #box{width:100px;height:100px;background-color:#000000;\
            transition:background-color 100ms linear}\
            #box.lit{background-color:#ffffff}\
            </style></head><body><div id=\"box\"></div></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 200, height: 200 };
        let (mut page, first) = LivePage::load("about:blank", html, &rect, None);
        assert_eq!(scene_background(&first, "box").as_deref(), Some("#000000"));

        let _ = page
            .host
            .eval_to_string("document.getElementById('box').className='lit'");

        let mut colors = Vec::new();
        for _ in 0..40 {
            if let Some(scene) = page.animation_frame(&rect) {
                if let Some(color) = scene_background(&scene, "box") {
                    colors.push(color);
                }
            }
            if !page.has_pending_animation() {
                break;
            }
        }

        assert!(
            colors.iter().any(|c| c != "#000000" && c != "#ffffff"),
            "expected intermediate greys, got {colors:?}"
        );
        assert_eq!(
            colors.last().map(String::as_str),
            Some("#ffffff"),
            "the transition must settle on the target, got {colors:?}"
        );
    }

    #[test]
    fn click_dispatch_runs_a_page_handler_and_relayouts() {
        // A real click hit-tests the retained layout, fires the listener on the
        // enclosing element, and re-lays-out the DOM the handler mutated.
        let html = "<html><head><style>body{margin:0}\
            #btn{width:100px;height:50px;background:#00ff00}</style></head><body>\
            <div id=\"btn\">press</div><p id=\"out\"></p>\
            <script>document.getElementById('btn').addEventListener('click',function(e){\
              document.getElementById('out').textContent='clicked';e.preventDefault();});\
            </script></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 300, height: 300 };
        let (mut page, first) = LivePage::load("about:blank", html, &rect, None);
        assert!(!scene_contains_text(&first, "clicked"));

        let outcome = page.dispatch_click((50, 25), &rect);
        assert!(outcome.hit, "the click should land on #btn");
        assert!(outcome.default_prevented, "the handler called preventDefault");
        let scene = outcome.scene.expect("the DOM mutation should re-layout");
        assert!(scene_contains_text(&scene, "clicked"));
    }

    #[test]
    fn click_on_a_link_posts_a_navigate_request() {
        // Documents are served through `prepare_html_for_display`, which injects
        // a click shim that turns anchor activation into a `postMessage`. A
        // dispatched click must reach it — the shim gates on `event.button`, so
        // this also pins the synthesized event carrying the mouse fields.
        let html = "<html><body style=\"margin:0\">\
            <a href=\"target.html\" style=\"font-size:40px\">GO</a></body></html>";
        let prepared =
            crate::loader::prepare_html_for_display(html, "http://example.test/link.html", "root");
        let rect = FrameRect { x: 0, y: 0, width: 400, height: 300 };
        let (mut page, _) = LivePage::load("http://example.test/link.html", &prepared, &rect, None);

        let outcome = page.dispatch_click((25, 25), &rect);
        assert!(outcome.hit);
        assert!(
            outcome.default_prevented,
            "the shim must preventDefault so the host owns navigation"
        );
        let request = outcome
            .messages
            .iter()
            .find_map(|m| crate::loader::parse_navigate_message(m))
            .expect("a navigate request should have been posted");
        assert_eq!(request.href, "target.html");
        assert_eq!(request.frame_id, "root");
        assert_eq!(request.target, None);
    }

    #[test]
    fn hover_restyles_and_can_start_a_transition() {
        // `:hover` is the commonest transition trigger. Pointing at the box
        // must re-style it (and start the declared transition); moving away
        // must return it to the base style.
        let html = "<html><head><style>body{margin:0}\
            #box{width:100px;height:100px;background-color:#000000;\
            transition:background-color 100ms linear}\
            #box:hover{background-color:#ffffff}</style></head>\
            <body><div id=\"box\"></div></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 300, height: 300 };
        let (mut page, first) = LivePage::load("about:blank", html, &rect, None);
        assert!(page.uses_hover());
        assert_eq!(scene_background(&first, "box").as_deref(), Some("#000000"));

        // Pointer enters: the target moves, so a transition starts and this
        // frame still paints the old value.
        let scene = page
            .set_hover_point(Some((50, 50)), &rect)
            .expect("entering the box must re-style it");
        assert_eq!(scene_background(&scene, "box").as_deref(), Some("#000000"));
        assert!(page.has_pending_animation(), "the hover transition should run");

        // Same element: no change, no work.
        assert!(
            page.set_hover_point(Some((60, 60)), &rect).is_none(),
            "moving within the same element must not re-style"
        );

        // Let it run to completion, then leave: it transitions back.
        for _ in 0..40 {
            if page.animation_frame(&rect).is_none() && !page.has_pending_animation() {
                break;
            }
        }
        let settled = page.relayout(&rect);
        assert_eq!(scene_background(&settled, "box").as_deref(), Some("#ffffff"));

        page.set_hover_point(None, &rect)
            .expect("leaving must re-style");
        assert!(page.has_pending_animation(), "leaving transitions back");
    }

    /// Text color of the scene text item containing `needle`.
    fn scene_text_color(scene: &LayoutScene, needle: &str) -> Option<String> {
        scene.scene_items.iter().find_map(|item| match item {
            SceneItem::Text { text, color, .. } if text.contains(needle) => Some(color.clone()),
            _ => None,
        })
    }

    #[test]
    fn color_transition_interpolates_and_inherits_to_children() {
        // `color` inherits, so an animating element must drag along descendants
        // that declared no color of their own — the child's text has to fade
        // with the parent even though only the parent has the transition.
        let html = "<html><head><style>body{margin:0}\
            #wrap{color:#000000;transition:color 100ms linear}\
            #wrap.lit{color:#ffffff}</style></head>\
            <body><div id=\"wrap\">PARENT<span>CHILD</span></div></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 300, height: 300 };
        let (mut page, first) = LivePage::load("about:blank", html, &rect, None);
        assert_eq!(scene_text_color(&first, "PARENT").as_deref(), Some("#000000"));
        assert_eq!(scene_text_color(&first, "CHILD").as_deref(), Some("#000000"));

        let _ = page
            .host
            .eval_to_string("document.getElementById('wrap').className='lit'");

        let mut midpoints = Vec::new();
        for _ in 0..40 {
            if let Some(scene) = page.animation_frame(&rect) {
                let parent = scene_text_color(&scene, "PARENT");
                let child = scene_text_color(&scene, "CHILD");
                assert_eq!(parent, child, "the child must inherit the animated color");
                if let Some(c) = parent {
                    if c != "#000000" && c != "#ffffff" {
                        midpoints.push(c);
                    }
                }
            }
            if !page.has_pending_animation() {
                break;
            }
        }
        assert!(!midpoints.is_empty(), "expected interpolated colors");
        let settled = page.relayout(&rect);
        assert_eq!(scene_text_color(&settled, "CHILD").as_deref(), Some("#ffffff"));
    }

    /// Width of the scene rect belonging to element `#id`.
    fn scene_width(scene: &LayoutScene, id: &str) -> Option<i64> {
        scene.scene_items.iter().find_map(|item| match item {
            SceneItem::Rect { anchor_id, width, .. } if anchor_id.as_deref() == Some(id) => {
                Some(*width)
            }
            _ => None,
        })
    }

    #[test]
    fn width_transition_relayouts_each_frame() {
        // A length transition changes layout, not just paint. The override
        // replaces the used width at its source, so clamps and layout see the
        // animated value with no extra plumbing.
        let html = "<html><head><style>body{margin:0}\
            #bar{width:100px;height:20px;background:#ff0000;transition:width 100ms linear}\
            #bar.wide{width:300px}</style></head>\
            <body><div id=\"bar\"></div></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 400, height: 300 };
        let (mut page, first) = LivePage::load("about:blank", html, &rect, None);
        assert_eq!(scene_width(&first, "bar"), Some(100));

        let _ = page
            .host
            .eval_to_string("document.getElementById('bar').className='wide'");

        let mut widths = Vec::new();
        for _ in 0..40 {
            if let Some(scene) = page.animation_frame(&rect) {
                if let Some(w) = scene_width(&scene, "bar") {
                    widths.push(w);
                }
            }
            if !page.has_pending_animation() {
                break;
            }
        }
        assert!(
            widths.windows(2).all(|w| w[1] >= w[0]),
            "width must grow monotonically, got {widths:?}"
        );
        assert!(
            widths.iter().any(|w| *w > 100 && *w < 300),
            "expected intermediate widths, got {widths:?}"
        );
        assert_eq!(widths.last().copied(), Some(300), "must settle on the target");
    }

    #[test]
    fn auto_width_never_starts_a_transition() {
        // `auto` has no numeric value to interpolate from, so a declared
        // `transition: width` on an auto-sized box must stay inert.
        let html = "<html><head><style>body{margin:0}\
            #b{height:10px;transition:width 1s}#b.w{background:#00ff00}</style></head>\
            <body><div id=\"b\"></div></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 400, height: 300 };
        let (mut page, _) = LivePage::load("about:blank", html, &rect, None);
        let _ = page.host.eval_to_string("document.getElementById('b').className='w'");
        page.animation_frame(&rect);
        assert!(!page.has_pending_animation(), "auto width is not interpolable");
    }

    #[test]
    fn delayed_timer_fires_on_the_frame_clock_not_at_load() {
        // A retained page owns a frame clock, so a delayed timer must wait
        // rather than being flushed before the first paint — otherwise timed
        // UI (and any transition it triggers) snaps to its end state.
        let html = "<html><head><style>body{margin:0}\
            #box{width:50px;height:50px;background-color:#000000;\
            transition:background-color 100ms linear}\
            #box.lit{background-color:#ffffff}</style></head><body>\
            <div id=\"box\"></div><script>setTimeout(function(){\
              document.getElementById('box').className='lit';},100);</script>\
            </body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 200, height: 200 };
        let (mut page, first) = LivePage::load("about:blank", html, &rect, None);
        assert_eq!(
            scene_background(&first, "box").as_deref(),
            Some("#000000"),
            "the timer must not have fired before the first paint"
        );

        // ~7 frames of 16ms reach the 100ms deadline; the class change then
        // starts the transition, which takes another 100ms.
        let mut colors = Vec::new();
        for _ in 0..40 {
            if let Some(scene) = page.animation_frame(&rect) {
                if let Some(color) = scene_background(&scene, "box") {
                    colors.push(color);
                }
            }
            if !page.has_pending_animation() {
                break;
            }
        }
        assert!(
            colors.iter().any(|c| c != "#000000" && c != "#ffffff"),
            "the timer should fire on the clock and transition, got {colors:?}"
        );
        assert_eq!(colors.last().map(String::as_str), Some("#ffffff"));
    }

    #[test]
    fn keyframes_animation_plays_and_holds_with_fill_forwards() {
        // A declared `@keyframes` plays on its own — no target change needed —
        // and `forwards` holds the last frame after the iteration count runs
        // out (without asking the GUI to keep driving frames).
        let html = "<html><head><style>body{margin:0}\
            @keyframes grow{from{width:100px}to{width:300px}}\
            #bar{width:100px;height:20px;background:#ff0000;\
            animation:grow 100ms linear forwards}</style></head>\
            <body><div id=\"bar\"></div></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 400, height: 300 };
        let (mut page, first) = LivePage::load("about:blank", html, &rect, None);
        assert_eq!(scene_width(&first, "bar"), Some(100), "starts at the first frame");
        assert!(page.has_pending_animation(), "playback starts on its own");

        let mut widths = Vec::new();
        for _ in 0..40 {
            if let Some(scene) = page.animation_frame(&rect) {
                if let Some(w) = scene_width(&scene, "bar") {
                    widths.push(w);
                }
            }
            if !page.has_pending_animation() {
                break;
            }
        }
        assert!(
            widths.windows(2).all(|w| w[1] >= w[0]),
            "must grow monotonically, got {widths:?}"
        );
        assert!(
            widths.iter().any(|w| *w > 100 && *w < 300),
            "expected intermediate widths, got {widths:?}"
        );
        assert_eq!(widths.last().copied(), Some(300), "fill:forwards holds the end");
        assert!(!page.has_pending_animation(), "a finished animation idles");
    }

    #[test]
    fn keyframes_alternate_direction_returns_to_the_start() {
        // `alternate` runs the second iteration backwards, so two iterations
        // end where the first began.
        let html = "<html><head><style>body{margin:0}\
            @keyframes pulse{from{background-color:#000000}to{background-color:#ffffff}}\
            #box{width:50px;height:50px;background-color:#000000;\
            animation:pulse 100ms linear 2 alternate}</style></head>\
            <body><div id=\"box\"></div></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 200, height: 200 };
        let (mut page, _) = LivePage::load("about:blank", html, &rect, None);

        let mut colors = Vec::new();
        for _ in 0..40 {
            if let Some(scene) = page.animation_frame(&rect) {
                if let Some(c) = scene_background(&scene, "box") {
                    colors.push(c);
                }
            }
            if !page.has_pending_animation() {
                break;
            }
        }
        assert!(
            colors.iter().any(|c| c != "#000000" && c != "#ffffff"),
            "expected interpolated greys, got {colors:?}"
        );
        // No fill mode, so after the last iteration the cascade value rules.
        let settled = page.relayout(&rect);
        assert_eq!(scene_background(&settled, "box").as_deref(), Some("#000000"));
    }

    #[test]
    fn infinite_animation_keeps_the_frame_clock_alive() {
        let html = "<html><head><style>\
            @keyframes blink{from{opacity:1}to{opacity:0}}\
            #d{width:10px;height:10px;animation:blink 50ms linear infinite}\
            </style></head><body><div id=\"d\"></div></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 200, height: 200 };
        let (mut page, _) = LivePage::load("about:blank", html, &rect, None);
        for _ in 0..30 {
            page.animation_frame(&rect);
        }
        assert!(
            page.has_pending_animation(),
            "an infinite animation never stops asking for frames"
        );
    }

    #[test]
    fn page_without_hover_rules_reports_no_hover_tracking() {
        let html = "<html><head><style>p{color:red}</style></head><body><p>x</p></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 300, height: 300 };
        let (page, _) = LivePage::load("about:blank", html, &rect, None);
        assert!(!page.uses_hover(), "no :hover rule — the GUI can skip tracking");
    }

    #[test]
    fn typing_into_a_focused_field_updates_it_and_fires_input() {
        // Clicking a text field focuses it, keystrokes change its value, the
        // page sees an `input` event, and the new value is what gets painted.
        let html = "<html><head><style>body{margin:0}\
            input{width:200px;height:24px}</style></head><body>\
            <input id=\"q\" value=\"\"><p id=\"echo\"></p>\
            <script>document.getElementById('q').addEventListener('input',function(e){\
              document.getElementById('echo').textContent='saw:'+e.target.getAttribute('value');});\
            </script></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 400, height: 200 };
        let (mut page, _) = LivePage::load("about:blank", html, &rect, None);

        assert!(page.focus_field_at((20, 10), &rect), "the click should focus the field");
        assert!(page.has_focused_field());

        let mut scene = None;
        for ch in ["h", "i", "!"] {
            scene = page.insert_text(ch, &rect);
        }
        let scene = scene.expect("typing should re-lay-out");
        assert!(
            scene_contains_text(&scene, "hi!"),
            "the field paints its new value"
        );
        assert!(
            scene_contains_text(&scene, "saw:hi!"),
            "the page's input handler ran and saw the value"
        );

        let scene = page.backspace(&rect).expect("backspace re-lays-out");
        assert!(scene_contains_text(&scene, "hi"), "backspace removes a character");
        assert!(!scene_contains_text(&scene, "hi!"));
    }

    #[test]
    fn focus_pseudo_classes_style_the_focused_field() {
        // `:focus` matches only the field; `:focus-within` also matches the
        // form around it. Both used to be poisoned to "never matches".
        let html = "<html><head><style>body{margin:0}\
            input{width:120px;height:20px;background:#ffffff}\
            input:focus{background:#ffff00}\
            form{background:#eeeeee}\
            form:focus-within{background:#00ff00}</style></head><body>\
            <form><input id=\"q\" value=\"\"></form></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 300, height: 200 };
        let (mut page, first) = LivePage::load("about:blank", html, &rect, None);
        assert!(page.uses_focus(), "the sheet styles on :focus");
        let backgrounds = |scene: &LayoutScene| -> Vec<String> {
            scene
                .scene_items
                .iter()
                .filter_map(|item| match item {
                    SceneItem::Rect { background_color, .. } => Some(background_color.clone()),
                    _ => None,
                })
                .collect()
        };
        assert!(!backgrounds(&first).contains(&"#ffff00".to_string()));
        assert!(!backgrounds(&first).contains(&"#00ff00".to_string()));

        assert!(page.focus_field_at((10, 10), &rect));
        let focused = page.relayout_for_focus(&rect).expect("focus re-lays-out");
        let colors = backgrounds(&focused);
        assert!(colors.contains(&"#ffff00".to_string()), ":focus applies to the field");
        assert!(colors.contains(&"#00ff00".to_string()), ":focus-within applies to the form");

        page.blur_field();
        let blurred = page.relayout_for_focus(&rect).expect("blur re-lays-out");
        let colors = backgrounds(&blurred);
        assert!(!colors.contains(&"#ffff00".to_string()), "focus styling is dropped");
        assert!(!colors.contains(&"#00ff00".to_string()));
    }

    #[test]
    fn clicking_outside_a_field_clears_the_focus() {
        let html = "<html><head><style>body{margin:0}\
            input{width:100px;height:20px}#pad{height:80px}</style></head><body>\
            <input value=\"x\"><div id=\"pad\">not a field</div></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 300, height: 200 };
        let (mut page, _) = LivePage::load("about:blank", html, &rect, None);
        assert!(page.focus_field_at((10, 10), &rect));
        assert!(!page.focus_field_at((10, 60), &rect), "the div is not a field");
        assert!(!page.has_focused_field());
        // With nothing focused, keystrokes must not change the page.
        assert!(page.insert_text("z", &rect).is_none());
    }

    #[test]
    fn typing_into_a_textarea_edits_its_text_content() {
        // A `<textarea>` keeps its value in child text rather than an
        // attribute (HTML LS §4.10.11), so editing has to go there — and its
        // existing text is the starting value, not something to discard.
        let html = "<html><head><style>body{margin:0}\
            textarea{width:200px;height:60px}</style></head><body>\
            <textarea id=\"n\">seed</textarea></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 300, height: 200 };
        let (mut page, first) = LivePage::load("about:blank", html, &rect, None);
        assert!(scene_contains_text(&first, "seed"), "starts with its text content");

        assert!(page.focus_field_at((190, 10), &rect), "a textarea takes focus");
        let scene = page.insert_text("ed", &rect).expect("typing re-lays-out");
        assert!(
            scene_contains_text(&scene, "seeded"),
            "typing appends to the existing text"
        );

        let scene = page.backspace(&rect).expect("backspace re-lays-out");
        assert!(scene_contains_text(&scene, "seede"));
    }

    #[test]
    fn typing_a_character_at_a_time_into_a_textarea_does_not_insert_spaces() {
        // The GUI delivers one keystroke per call, so each edit re-reads the
        // value it just wrote. Anything that mangles the round-trip shows up
        // here and not in a single multi-character insert.
        let html = "<html><head><style>body{margin:0}\
            textarea{width:200px;height:60px}</style></head><body>\
            <p>label</p><textarea id=\"n\">seed text</textarea>\n\
            <p>after</p></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 300, height: 200 };
        let (mut page, _) = LivePage::load("about:blank", html, &rect, None);
        assert!(page.focus_field_at((190, 70), &rect), "textarea takes focus");
        let mut scene = None;
        // Two things this pins, both of which bit in the GUI but not in a
        // tidier fixture: a leading space (what the GUI sends for the space
        // key), and content *after* the textarea — reading the value used to
        // sweep up the following siblings' text, so the newline after
        // `</textarea>` joined the value and every keystroke landed on its
        // own line.
        for ch in " + typed".chars() {
            scene = page.insert_text(&ch.to_string(), &rect);
        }
        let scene = scene.expect("typing re-lays-out");
        assert!(
            scene_contains_text(&scene, "seed text + typed"),
            "characters land next to each other, not spaced apart"
        );
        assert!(
            scene_contains_text(&scene, "after"),
            "the following sibling is untouched"
        );
    }

    #[test]
    fn typing_into_an_empty_textarea_creates_its_text() {
        let html = "<html><head><style>body{margin:0}\
            textarea{width:200px;height:60px}</style></head><body>\
            <textarea id=\"n\"></textarea></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 300, height: 200 };
        let (mut page, _) = LivePage::load("about:blank", html, &rect, None);
        assert!(page.focus_field_at((20, 10), &rect));
        let scene = page.insert_text("new", &rect).expect("typing re-lays-out");
        assert!(scene_contains_text(&scene, "new"), "a text child is created");
    }

    fn caret_page(body: &str) -> (LivePage, FrameRect) {
        let html = format!(
            "<html><head><style>body{{margin:0}}input{{width:200px;height:24px}}\
             textarea{{width:300px;height:100px}}</style></head><body>{body}</body></html>"
        );
        let rect = FrameRect { x: 0, y: 0, width: 400, height: 200 };
        let (page, _) = LivePage::load("about:blank", &html, &rect, None);
        (page, rect)
    }

    #[test]
    fn typing_and_deleting_happen_at_the_caret() {
        let (mut page, rect) = caret_page("<input value=\"abcd\">");
        assert!(page.focus_field_at((190, 10), &rect));
        assert!(page.move_caret(CaretMove::Left, &rect));
        let scene = page.insert_text("X", &rect).expect("typing re-lays-out");
        assert!(scene_contains_text(&scene, "abcXd"), "inserted before the last character");

        let scene = page.backspace(&rect).expect("backspace re-lays-out");
        assert!(scene_contains_text(&scene, "abcd"), "backspace takes the character before");

        let scene = page.delete_forward(&rect).expect("delete re-lays-out");
        assert!(scene_contains_text(&scene, "abc"), "delete takes the character after");
        assert!(!scene_contains_text(&scene, "abcd"));

        assert!(page.move_caret(CaretMove::LineStart, &rect));
        let scene = page.insert_text("^", &rect).expect("typing re-lays-out");
        assert!(scene_contains_text(&scene, "^abc"));
    }

    #[test]
    fn the_caret_handles_multibyte_characters() {
        let (mut page, rect) = caret_page("<input value=\"日本語\">");
        assert!(page.focus_field_at((190, 10), &rect));
        page.move_caret(CaretMove::Left, &rect);
        let scene = page.insert_text("a", &rect).expect("typing re-lays-out");
        assert!(scene_contains_text(&scene, "日本a語"));
        let scene = page.backspace(&rect).expect("backspace re-lays-out");
        assert!(scene_contains_text(&scene, "日本語"));
        let scene = page.backspace(&rect).expect("backspace re-lays-out");
        assert!(scene_contains_text(&scene, "日語"));
    }

    #[test]
    fn an_edit_that_changes_nothing_fires_no_input_event() {
        let (mut page, rect) = caret_page(
            "<input id=\"q\" value=\"\"><p id=\"seen\">none</p><script>\
             document.getElementById('q').addEventListener('input',function(){\
             document.getElementById('seen').textContent='fired';});</script>",
        );
        assert!(page.focus_field_at((10, 10), &rect));
        assert!(page.backspace(&rect).is_none(), "nothing before the caret");
        assert!(page.delete_forward(&rect).is_none(), "nothing after the caret");
        assert!(!page.move_caret(CaretMove::Left, &rect), "already at the start");
        assert!(!page.move_caret(CaretMove::Right, &rect), "already at the end");
        let scene = page.relayout(&rect);
        assert!(scene_contains_text(&scene, "none"), "the handler never ran");
    }

    #[test]
    fn clicking_places_the_caret_nearest_the_point() {
        let (mut page, rect) = caret_page("<input value=\"abcdefgh\">");
        assert!(page.focus_field_at((10, 10), &rect), "left of the text");
        let start = page.caret_rect(&rect).expect("a caret").x;
        assert!(page.focus_field_at((190, 10), &rect), "right of the text");
        let end = page.caret_rect(&rect).expect("a caret").x;
        assert!(end > start);
        assert!(page.focus_field_at(((start + end) / 2, 10), &rect), "the middle");
        let middle = page.caret_rect(&rect).expect("a caret").x;
        assert!(start < middle && middle < end, "{start} < {middle} < {end}");
    }

    #[test]
    fn the_caret_follows_lines_in_a_textarea() {
        let (mut page, rect) = caret_page("<textarea>one\n\nthree</textarea>");
        assert!(page.focus_field_at((290, 90), &rect), "below the text: the end");
        let last = page.caret_rect(&rect).expect("a caret");

        page.move_caret(CaretMove::Up, &rect);
        let blank = page.caret_rect(&rect).expect("a caret");
        assert!(blank.y < last.y, "up leaves the last line");
        page.move_caret(CaretMove::Up, &rect);
        let first = page.caret_rect(&rect).expect("a caret");
        assert!(first.y < blank.y, "the blank line is a line of its own");

        page.move_caret(CaretMove::LineStart, &rect);
        let scene = page.insert_text(">", &rect).expect("typing re-lays-out");
        assert!(scene_contains_text(&scene, ">one"));

        page.move_caret(CaretMove::Down, &rect);
        page.move_caret(CaretMove::Down, &rect);
        page.move_caret(CaretMove::LineEnd, &rect);
        let scene = page.insert_text("!", &rect).expect("typing re-lays-out");
        assert!(scene_contains_text(&scene, "three!"));

        page.move_caret(CaretMove::Down, &rect);
        let scene = page.insert_text("?", &rect).expect("typing re-lays-out");
        assert!(scene_contains_text(&scene, "three!?"), "down on the last line goes to the end");
    }

    #[test]
    fn a_newline_starts_a_new_line_with_the_caret_on_it() {
        let (mut page, rect) = caret_page("<textarea></textarea>");
        assert!(page.focus_field_at((10, 10), &rect));
        assert!(page.focused_field_is_multiline());
        let scene = page.insert_text("a\n", &rect).expect("typing re-lays-out");
        assert!(scene_contains_text(&scene, "a"));
        let on_new_line = page.caret_rect(&rect).expect("a caret");
        let scene = page.insert_text("b", &rect).expect("typing re-lays-out");
        let painted_y = |needle: &str| {
            scene.scene_items.iter().find_map(|item| match item {
                SceneItem::Text { text, y, .. } if text == needle => Some(*y),
                _ => None,
            })
        };
        let (a, b) = (painted_y("a").expect("a painted"), painted_y("b").expect("b painted"));
        assert!(b > a, "the newline moved the text down");
        assert_eq!(on_new_line.y, b, "the caret was already on that line");
    }

    #[test]
    fn a_single_line_input_is_not_multiline_and_a_script_can_shorten_its_value() {
        let (mut page, rect) = caret_page("<input id=\"q\" value=\"abcdef\">");
        assert!(page.focus_field_at((190, 10), &rect));
        assert!(!page.focused_field_is_multiline());
        page.host
            .eval_to_string("document.getElementById('q').setAttribute('value','ab');")
            .expect("script runs");
        page.relayout(&rect);
        let scene = page.insert_text("c", &rect).expect("typing re-lays-out");
        assert!(scene_contains_text(&scene, "abc"), "the caret was clamped to the new end");
    }

    #[test]
    fn a_button_input_does_not_take_keystrokes() {
        let html = "<html><body><input type=\"submit\" value=\"Go\"></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 300, height: 200 };
        let (mut page, _) = LivePage::load("about:blank", html, &rect, None);
        assert!(!page.focus_field_at((10, 10), &rect), "submit is not a text field");
    }

    #[test]
    fn click_on_empty_space_dispatches_nothing() {
        let html = "<html><body><div id=\"d\" style=\"width:10px;height:10px\"></div></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 300, height: 300 };
        let (mut page, _) = LivePage::load("about:blank", html, &rect, None);
        // Far below any generated box.
        let outcome = page.dispatch_click((280, 290), &rect);
        assert!(!outcome.hit);
        assert!(!outcome.default_prevented);
        assert!(outcome.scene.is_none());
    }

    fn page_with(html: &str) -> (LivePage, FrameRect) {
        let rect = FrameRect { x: 0, y: 0, width: 400, height: 300 };
        let (page, _) = LivePage::load("http://x.test/dir/page", html, &rect, None);
        (page, rect)
    }

    fn element(page: &LivePage, id: &str) -> Rc<RefCell<cosmo_engine::renderer::dom::node::Node>> {
        cosmo_engine::renderer::dom::api::get_element_by_id(
            Some(page.dom.clone()),
            &id.to_string(),
        )
        .expect("element")
    }

    fn has_attribute(page: &LivePage, id: &str, name: &str) -> bool {
        matches!(
            element(page, id).borrow().kind,
            NodeKind::Element(ref e) if e.get_attribute(name).is_some()
        )
    }

    const CONTROLS: &str = "<!doctype html><style>body{margin:0}input,button,select,summary{display:block;width:200px;height:20px;margin:0;padding:0;border:0}</style>";

    #[test]
    fn clicking_a_checkbox_toggles_it_and_repaints() {
        let html = format!("{CONTROLS}<input type=checkbox id=c>");
        let (mut page, rect) = page_with(&html);
        let outcome = page.dispatch_click((5, 5), &rect);
        assert!(has_attribute(&page, "c", "checked"));
        assert!(outcome.scene.is_some());
        page.dispatch_click((5, 5), &rect);
        assert!(!has_attribute(&page, "c", "checked"));
    }

    #[test]
    fn prevent_default_stops_the_activation() {
        let html = format!(
            "{CONTROLS}<input type=checkbox id=c><script>document.getElementById('c').addEventListener('click',function(e){{e.preventDefault()}})</script>"
        );
        let (mut page, rect) = page_with(&html);
        page.dispatch_click((5, 5), &rect);
        assert!(!has_attribute(&page, "c", "checked"));
    }

    #[test]
    fn a_change_event_follows_the_toggle() {
        let html = format!(
            "{CONTROLS}<input type=checkbox id=c><p id=o></p><script>document.getElementById('c').addEventListener('change',function(){{document.getElementById('o').textContent='changed'}})</script>"
        );
        let (mut page, rect) = page_with(&html);
        let outcome = page.dispatch_click((5, 5), &rect);
        assert!(scene_contains_text(&outcome.scene.expect("repaint"), "changed"));
    }

    #[test]
    fn a_select_opens_and_a_click_picks_an_option() {
        let html = format!("{CONTROLS}<select id=s><option id=a>first<option id=b>second</select>");
        let (mut page, rect) = page_with(&html);
        page.dispatch_click((5, 5), &rect);
        assert!(has_attribute(&page, "s", "data-cosmo-open"), "the list opens");
        // Rows start below the 20px box, and the second comes after one row.
        let row = 20 + styled_row_height(&page, &rect, "s") + 2;
        page.dispatch_click((5, row), &rect);
        assert!(!has_attribute(&page, "s", "data-cosmo-open"));
        assert!(has_attribute(&page, "b", "selected"));
        assert!(!has_attribute(&page, "a", "selected"));
    }

    fn styled_row_height(page: &LivePage, rect: &FrameRect, id: &str) -> i64 {
        let view = build_layout_view(page.dom.clone(), &page.cssom.borrow(), rect);
        let object = view.find_object_of_node(&element(page, id)).expect("box");
        let rows = object.borrow().select_popup_rows();
        rows[0].3
    }

    #[test]
    fn a_click_outside_an_open_select_only_closes_it() {
        let html = format!("{CONTROLS}<select id=s><option>first</select><input type=checkbox id=c>");
        let (mut page, rect) = page_with(&html);
        page.dispatch_click((5, 5), &rect);
        page.dispatch_click((300, 290), &rect);
        assert!(!has_attribute(&page, "s", "data-cosmo-open"));
        assert!(!has_attribute(&page, "c", "checked"));
    }

    #[test]
    fn clicking_a_summary_toggles_its_details() {
        let html = format!("{CONTROLS}<details id=d><summary>head</summary><p>body</p></details>");
        let (mut page, rect) = page_with(&html);
        let outcome = page.dispatch_click((5, 5), &rect);
        assert!(has_attribute(&page, "d", "open"));
        assert!(scene_contains_text(&outcome.scene.expect("repaint"), "body"));
    }

    #[test]
    fn a_submit_button_asks_to_navigate_with_the_form_data() {
        let html = format!(
            "{CONTROLS}<form action=results><input type=submit name=go value=Go></form>"
        );
        let (mut page, rect) = page_with(&html);
        let outcome = page.dispatch_click((5, 5), &rect);
        let request = outcome
            .messages
            .iter()
            .find_map(|m| crate::loader::parse_navigate_message(m))
            .expect("a navigation");
        assert_eq!(request.href, "http://x.test/dir/results?go=Go");
        assert_eq!(request.post_body, None);
    }

    #[test]
    fn a_post_form_keeps_its_data_out_of_the_url() {
        let html = format!(
            "{CONTROLS}<form action=login method=post><input type=submit name=go value=Go></form>"
        );
        let (mut page, rect) = page_with(&html);
        let outcome = page.dispatch_click((5, 5), &rect);
        let request = outcome
            .messages
            .iter()
            .find_map(|m| crate::loader::parse_navigate_message(m))
            .expect("a navigation");
        assert_eq!(request.href, "http://x.test/dir/login");
        assert_eq!(request.post_body.as_deref(), Some("go=Go"));
    }

    #[test]
    fn a_page_can_cancel_a_submission() {
        let html = format!(
            "{CONTROLS}<form id=f action=x><input type=submit value=Go></form><script>document.getElementById('f').addEventListener('submit',function(e){{e.preventDefault()}})</script>"
        );
        let (mut page, rect) = page_with(&html);
        let outcome = page.dispatch_click((5, 5), &rect);
        assert!(outcome
            .messages
            .iter()
            .all(|m| crate::loader::parse_navigate_message(m).is_none()));
    }

    #[test]
    fn enter_in_a_field_submits_its_form() {
        let html = format!("{CONTROLS}<form action=s><input name=q value=hi></form>");
        let (mut page, rect) = page_with(&html);
        assert!(page.focus_field_at((5, 5), &rect));
        let messages = page.submit_focused_field();
        let request = messages
            .iter()
            .find_map(|m| crate::loader::parse_navigate_message(m))
            .expect("a navigation");
        assert_eq!(request.href, "http://x.test/dir/s?q=hi");
    }

    #[test]
    fn a_password_never_paints_what_was_typed() {
        let html = format!("{CONTROLS}<input type=password id=p value=secret>");
        let (_, rect) = page_with(&html);
        let (page, scene) = LivePage::load("http://x.test/", &html, &rect, None);
        let _ = page;
        assert!(!scene_contains_text(&scene, "secret"));
        assert!(scene_contains_text(&scene, "\u{2022}\u{2022}\u{2022}"));
    }

    #[test]
    fn typing_into_a_password_field_keeps_it_masked() {
        let html = format!("{CONTROLS}<input type=password id=p>");
        let (mut page, rect) = page_with(&html);
        assert!(page.focus_field_at((5, 5), &rect));
        let scene = page.insert_text("abc", &rect).expect("repaint");
        assert!(!scene_contains_text(&scene, "abc"));
        assert!(scene_contains_text(&scene, "\u{2022}\u{2022}\u{2022}"));
    }

    #[test]
    fn a_field_with_nothing_in_it_shows_its_placeholder_again() {
        let html = format!("{CONTROLS}<input id=i placeholder=hint value=a>");
        let (mut page, rect) = page_with(&html);
        assert!(page.focus_field_at((5, 5), &rect));
        let scene = page.backspace(&rect).expect("repaint");
        assert!(scene_contains_text(&scene, "hint"));
    }

    #[test]
    fn an_empty_field_paints_no_stand_in_text() {
        let html = format!("{CONTROLS}<input id=i>");
        let rect = FrameRect { x: 0, y: 0, width: 400, height: 300 };
        let (_, scene) = LivePage::load("http://x.test/", &html, &rect, None);
        assert!(!scene_contains_text(&scene, "Input"));
    }

    fn scene_contains_text(scene: &LayoutScene, needle: &str) -> bool {
        scene
            .scene_items
            .iter()
            .any(|item| matches!(item, SceneItem::Text { text, .. } if text.contains(needle)))
    }

    #[test]
    fn static_page_never_starts_a_transition() {
        // A page that declares a transition but never changes the target must
        // not animate — no frame clock, no repaints (static-page non-regression).
        let html = "<html><head><style>#box{width:10px;height:10px;opacity:0.5;\
            transition:opacity 1s}</style></head><body><div id=\"box\"></div></body></html>";
        let rect = FrameRect { x: 0, y: 0, width: 200, height: 200 };
        let (mut page, scene) = LivePage::load("about:blank", html, &rect, None);
        assert_eq!(scene_opacity(&scene, "box"), Some(0.5));
        assert!(!page.has_pending_animation());
        assert!(page.animation_frame(&rect).is_none());
    }

    #[test]
    fn pump_without_dom_mutation_skips_relayout() {
        let _env = lock_layout_assert_env();
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
