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

/// Upper bound on the concatenated `<script>` bytes we hand to Boa. The
/// execution watchdog (loop/recursion caps in cosmo_script) prevents runaway
/// *execution*, but Boa still has to *parse* the whole payload up front, so a
/// cap keeps navigation responsive on pages shipping multi-MB minified bundles
/// the engine can't meaningfully run anyway. Raised well above typical page JS
/// now that execution is bounded (plan 3.5 wanted this removed; a parse-time
/// guard remains prudent).
const MAX_SCRIPT_BYTES: usize = 2 * 1024 * 1024;

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
    /// The parsed stylesheets, resolved once at load and reused across
    /// pumps/reflows (script DOM mutations rarely change the stylesheets, so
    /// the expensive CSS re-parse is skipped — the first incremental win).
    cssom: cosmo_engine::renderer::css::cssom::StyleSheet,
    /// DOM mutation generation at the last layout; a pump that leaves it
    /// unchanged skips re-layout (no script tick touched the DOM).
    last_generation: u64,
    /// Declarative CSS `transition` state (Phase 4.4): watches the cascade's
    /// target values across layouts and interpolates the in-between frames.
    transitions: TransitionDriver,
    /// `@keyframes` playback state (Phase 4.4): plays declared timelines.
    keyframes: KeyframeDriver,
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
        // The hover chain keys on node addresses, which a freed document can
        // hand back to a new one — drop it before the new DOM exists.
        cosmo_engine::renderer::style::values::set_hover_chain(&[]);
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
        if !script.trim().is_empty() && script.len() <= MAX_SCRIPT_BYTES {
            let _ = host.eval_to_string(&script);
            // The document is parsed by the time we get here, so the handlers
            // the scripts just registered for it fire now.
            host.fire_dom_content_loaded();
            // Only what is due at t=0. A retained page has a frame clock
            // (`animation_frame`) to deliver delayed timers on schedule, so
            // unlike the one-shot pipeline it must not flush them up front —
            // that would make timed UI (and the transitions it triggers) snap
            // to its end state before the first paint.
            host.run_due(1000);
        }
        replace_local_storage(document_url, &host.local_storage_entries());

        let cssom = resolve_cssom(dom.clone(), document_url);
        let last_generation = host.dom_generation();
        let mut page = Self {
            host,
            dom,
            document_url: document_url.to_string(),
            cssom,
            last_generation,
            transitions: TransitionDriver::default(),
            keyframes: KeyframeDriver::default(),
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
        let mut outcome = ClickOutcome::default();
        let target = {
            let view = build_layout_view(self.dom.clone(), &self.cssom, rect);
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
        outcome.default_prevented = !self.host.dispatch_mouse_event(target, "click", x, y);
        replace_local_storage(&self.document_url, &self.host.local_storage_entries());
        outcome.messages = self.host.take_posted_messages();

        let generation = self.host.dom_generation();
        if generation != self.last_generation {
            self.last_generation = generation;
            outcome.scene = Some(self.layout_and_drive(rect));
        }
        outcome
    }

    /// Whether this document has any `:hover` rule. Pointer motion over a page
    /// without one changes nothing, so the renderer can skip hover tracking
    /// (which costs a hit-test plus a re-style) entirely.
    pub fn uses_hover(&self) -> bool {
        self.cssom.uses_hover()
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
                    let view = build_layout_view(self.dom.clone(), &self.cssom, rect);
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

    /// Lay out the retained DOM and drive declarative transitions: whenever the
    /// cascade's target for a transitioned property moved since the previous
    /// layout, a transition starts and the interpolation's *start* value is
    /// written back onto the DOM — so this frame paints the old value instead
    /// of flashing the end state. That override costs one extra layout on the
    /// frames where a transition starts or is cancelled, never on static pages.
    fn layout_and_drive(&mut self, rect: &FrameRect) -> LayoutScene {
        let (scene, transitions, animations) =
            layout_scene_and_targets(self.dom.clone(), &self.cssom, rect);
        // `@keyframes` playback first: it may write overrides that the
        // transition driver should then see as the current displayed value.
        let animated = self.keyframes.sync(&animations);
        if self.transitions.sync_targets(&transitions) | animated {
            layout_scene_only(self.dom.clone(), &self.cssom, rect)
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
    let script = get_js_content(dom.clone());
    let mut diagnostics = Vec::new();
    let mut host = cosmo_script::ScriptHost::new();
    host.set_location(document_url);
    host.set_local_storage_entries(local_storage_snapshot(document_url));
    host.set_fetch_engine(crate::loader::make_fetch_engine(document_url));
    host.set_document(dom);

    // Interim watchdog (see fn doc). Larger than the toy cap since Boa handles
    // far more real-world JS, but still bounded.
    let ran = if !script.trim().is_empty() && script.len() <= MAX_SCRIPT_BYTES {
        if let Err(e) = host.eval_to_string(&script) {
            diagnostics.push(format!("Script error: {e}"));
        }
        // The document is parsed by now, so its DOMContentLoaded handlers run.
        host.fire_dom_content_loaded();
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
    fn incremental_matches_full_under_assert() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        // A fetch completes after load and its handler mutates the DOM; under
        // COSMO_LAYOUT_ASSERT the reused-CSSOM incremental pump must produce
        // scene items byte-identical to a full from-scratch layout.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            if let Some(Ok(mut stream)) = listener.incoming().next() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = r#"{"items":["row0","row1","row2"]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });

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
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut scene = None;
        while page.has_pending_work() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
            if let Some(s) = page.pump_and_relayout(&rect) {
                scene = Some(s);
            }
        }
        std::env::remove_var("COSMO_LAYOUT_ASSERT");
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
