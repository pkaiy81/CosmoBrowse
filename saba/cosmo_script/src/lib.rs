//! JavaScript engine integration (plan Phase 3). Wraps boa_engine so the
//! rest of the browser can evaluate scripts without depending on Boa's API
//! surface directly — DOM bindings and the event loop layer on top of this.

use boa_engine::{
    js_string,
    object::{FunctionObjectBuilder, JsObject, ObjectInitializer},
    property::{Attribute, PropertyDescriptor},
    Context, JsData, JsResult, JsValue, NativeFunction, Source,
};
use boa_engine::gc::{Finalize, Trace};
use boa_engine::object::builtins::{JsArray, JsPromise, JsProxy};
use boa_engine::builtins::promise::ResolvingFunctions;
use cosmo_engine::renderer::dom::api::{
    collect_text, element_closest, element_matches, get_element_by_id, get_target_element_node,
    query_selector, query_selector_all,
};
use cosmo_engine::renderer::dom::node::{Element, ElementKind, Node, NodeKind};
use cosmo_engine::renderer::html::{parser::HtmlParser, token::HtmlTokenizer};
use std::cell::RefCell;
use std::rc::{Rc, Weak};

/// Host data attached to an `Element` JsObject: a handle to the live DOM
/// node, kept outside Boa's GC. The Rc/RefCell contain no GC-managed
/// values, so ignoring them for tracing is sound.
#[derive(Trace, Finalize, JsData)]
struct NodeHandle {
    #[unsafe_ignore_trace]
    node: Rc<RefCell<Node>>,
}

/// Host data on an `Event` JsObject: the propagation/default flags. Held as
/// `Cell`s so the native `preventDefault` / `stopPropagation` methods can flip
/// them through a shared `&self` reference.
#[derive(Trace, Finalize, JsData)]
struct EventFlags {
    #[unsafe_ignore_trace]
    default_prevented: std::cell::Cell<bool>,
    #[unsafe_ignore_trace]
    stop_propagation: std::cell::Cell<bool>,
}

/// A registered event listener: the event type and its JS callback. The
/// callback is a `JsObject`, whose `Gc` handle keeps the function rooted while
/// it lives in the registry.
struct Listener {
    event_type: String,
    callback: JsObject,
    /// True if registered for the capture phase (addEventListener's 3rd arg
    /// `true` or `{capture: true}`).
    capture: bool,
}

/// Parse addEventListener's optional 3rd argument (a boolean `useCapture` or an
/// options object with a `capture` property).
fn parse_capture(arg: Option<&JsValue>, ctx: &mut Context) -> bool {
    match arg {
        Some(v) if v.is_object() => v
            .as_object()
            .and_then(|o| o.get(js_string!("capture"), ctx).ok())
            .map(|c| c.to_boolean())
            .unwrap_or(false),
        Some(v) => v.to_boolean(),
        None => false,
    }
}

/// A network request initiated by `fetch()` / `XMLHttpRequest`, handed to the
/// host's [`FetchEngine`]. Plain data so it can cross to a worker thread.
pub struct FetchRequest {
    pub url: String,
    pub method: String,
    pub body: Option<String>,
    /// Request headers (name, value) from fetch `options.headers` or
    /// `XMLHttpRequest.setRequestHeader`.
    pub headers: Vec<(String, String)>,
}

/// The result of a network request delivered back from the host's worker.
/// `Send` (plain data) so the worker thread can post it over a channel.
pub struct FetchResponse {
    pub ok: bool,
    pub status: u16,
    pub status_text: String,
    pub url: String,
    pub body: String,
    /// Set on a network-level failure (DNS, connection, CORS block); the
    /// promise rejects rather than resolving to a Response.
    pub error: Option<String>,
}

/// The host's network backend. `cosmo_script` stays network-free: it produces
/// the request and consumes the response, but the runtime supplies this to do
/// the actual IO (on a worker thread, with CORS/security enforcement). `start`
/// must return immediately with a receiver that yields exactly one response.
pub trait FetchEngine: 'static {
    fn start(&self, req: FetchRequest) -> std::sync::mpsc::Receiver<FetchResponse>;
}

/// A `fetch` awaiting its worker response: the channel plus the promise's
/// resolve/reject functions to settle once it arrives.
struct PendingFetch {
    rx: std::sync::mpsc::Receiver<FetchResponse>,
    resolvers: ResolvingFunctions,
}

/// Host data on an `XMLHttpRequest` object: the request configuration set
/// between `open()` and `send()`.
#[derive(Trace, Finalize, JsData, Default)]
struct XhrState {
    #[unsafe_ignore_trace]
    method: RefCell<String>,
    #[unsafe_ignore_trace]
    url: RefCell<String>,
    #[unsafe_ignore_trace]
    headers: RefCell<Vec<(String, String)>>,
}

/// An `XMLHttpRequest.send()` awaiting its worker response: the channel plus
/// the XHR object to update and fire callbacks on when it arrives.
struct PendingXhr {
    rx: std::sync::mpsc::Receiver<FetchResponse>,
    obj: JsObject,
}

/// A scheduled `setTimeout`/`setInterval`/`requestAnimationFrame` callback
/// awaiting its turn.
struct Timer {
    id: u32,
    callback: JsObject,
    /// Virtual fire time (accumulated delay), used only to order due timers.
    due: u64,
    /// Repeat interval in ms for `setInterval`; `None` for one-shot timers.
    interval: Option<u64>,
    /// True for `requestAnimationFrame` callbacks — they receive the current
    /// timestamp as their argument.
    is_raf: bool,
}

/// All per-page script state (plan D5). Each [`ScriptHost`] owns its own
/// `Rc<PageState>`; the active one is pointed to by the `ACTIVE_PAGE`
/// thread-local while that host runs (JS is single-threaded, so exactly one
/// host is active at a time, but each keeps independent state between runs —
/// this is what lets multiple LivePages coexist on one thread).
struct PageState {
    /// The document currently exposed to script (`document`). Stays a
    /// `Rc<RefCell<Node>>` outside Boa's GC; native fns read it from here.
    script_dom: RefCell<Option<Rc<RefCell<Node>>>>,
    /// Event listeners keyed by DOM node identity (`Rc::as_ptr`).
    listeners: RefCell<std::collections::HashMap<usize, Vec<Listener>>>,
    /// Element-wrapper cache keyed by node identity, so `el === el` holds.
    wrapper_cache: RefCell<std::collections::HashMap<usize, JsObject>>,
    /// Lines emitted by `console.*`, in order.
    console_log: RefCell<Vec<String>>,
    /// Pending timers (setTimeout/setInterval/rAF).
    timers: RefCell<Vec<Timer>>,
    next_timer_id: std::cell::Cell<u32>,
    /// Monotonic virtual clock advanced as timers fire.
    virtual_clock: std::cell::Cell<u64>,
    /// Bumped on every DOM mutation (see LivePage::pump_and_relayout).
    dom_generation: std::cell::Cell<u64>,
    /// The document URL exposed as `location`.
    location_href: RefCell<String>,
    /// `localStorage` backing store (insertion-ordered).
    local_storage: RefCell<Vec<(String, String)>>,
    /// Messages posted via `window.parent.postMessage` (JSON strings).
    posted_messages: RefCell<Vec<String>>,
    /// The host's network backend for `fetch`/XHR (None → requests reject).
    fetch_engine: RefCell<Option<Box<dyn FetchEngine>>>,
    /// In-flight `fetch` requests awaiting their worker response.
    pending_fetches: RefCell<Vec<PendingFetch>>,
    /// In-flight `XMLHttpRequest.send()` requests awaiting their response.
    pending_xhr: RefCell<Vec<PendingXhr>>,
}

impl Default for PageState {
    fn default() -> Self {
        Self {
            script_dom: RefCell::new(None),
            listeners: RefCell::new(std::collections::HashMap::new()),
            wrapper_cache: RefCell::new(std::collections::HashMap::new()),
            console_log: RefCell::new(Vec::new()),
            timers: RefCell::new(Vec::new()),
            next_timer_id: std::cell::Cell::new(1),
            virtual_clock: std::cell::Cell::new(0),
            dom_generation: std::cell::Cell::new(0),
            location_href: RefCell::new(String::from("about:blank")),
            local_storage: RefCell::new(Vec::new()),
            posted_messages: RefCell::new(Vec::new()),
            fetch_engine: RefCell::new(None),
            pending_fetches: RefCell::new(Vec::new()),
            pending_xhr: RefCell::new(Vec::new()),
        }
    }
}

thread_local! {
    /// The page whose state native functions currently see. A `ScriptHost`
    /// sets this to its own `PageState` before running any JS (`activate`).
    static ACTIVE_PAGE: RefCell<Option<Rc<PageState>>> = const { RefCell::new(None) };
}

/// The active page's shared state. Panics if no host has activated — every
/// public `ScriptHost` entry point activates before running JS.
fn active_page() -> Rc<PageState> {
    ACTIVE_PAGE.with(|p| {
        p.borrow()
            .clone()
            .expect("no active ScriptHost page (call a ScriptHost method)")
    })
}

/// Generates a zero-sized accessor named `$konst` with a thread-local-style
/// `.with(|&field| ...)` method routing to the active page's `$field`, so the
/// call sites read exactly like the previous `thread_local!` statics.
macro_rules! page_field {
    ($konst:ident, $field:ident, $ty:ty) => {
        #[allow(non_camel_case_types)]
        struct $konst;
        impl $konst {
            #[inline]
            fn with<R>(&self, f: impl FnOnce(&$ty) -> R) -> R {
                let ps = active_page();
                f(&ps.$field)
            }
        }
    };
}

page_field!(SCRIPT_DOM, script_dom, RefCell<Option<Rc<RefCell<Node>>>>);
page_field!(LISTENERS, listeners, RefCell<std::collections::HashMap<usize, Vec<Listener>>>);
page_field!(WRAPPER_CACHE, wrapper_cache, RefCell<std::collections::HashMap<usize, JsObject>>);
page_field!(CONSOLE_LOG, console_log, RefCell<Vec<String>>);
page_field!(TIMERS, timers, RefCell<Vec<Timer>>);
page_field!(NEXT_TIMER_ID, next_timer_id, std::cell::Cell<u32>);
page_field!(VIRTUAL_CLOCK, virtual_clock, std::cell::Cell<u64>);
page_field!(DOM_GENERATION, dom_generation, std::cell::Cell<u64>);
page_field!(LOCATION_HREF, location_href, RefCell<String>);
page_field!(LOCAL_STORAGE, local_storage, RefCell<Vec<(String, String)>>);
page_field!(POSTED_MESSAGES, posted_messages, RefCell<Vec<String>>);
page_field!(FETCH_ENGINE, fetch_engine, RefCell<Option<Box<dyn FetchEngine>>>);
page_field!(PENDING_FETCHES, pending_fetches, RefCell<Vec<PendingFetch>>);
page_field!(PENDING_XHR, pending_xhr, RefCell<Vec<PendingXhr>>);

fn ls_get(key: &str) -> Option<String> {
    LOCAL_STORAGE.with(|s| s.borrow().iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()))
}
fn ls_set(key: &str, value: &str) {
    LOCAL_STORAGE.with(|s| {
        let mut v = s.borrow_mut();
        if let Some(entry) = v.iter_mut().find(|(k, _)| k == key) {
            entry.1 = value.to_string();
        } else {
            v.push((key.to_string(), value.to_string()));
        }
    });
}
fn ls_remove(key: &str) {
    LOCAL_STORAGE.with(|s| s.borrow_mut().retain(|(k, _)| k != key));
}

/// Decompose a URL into (protocol, host, pathname, search, hash). Best-effort;
/// missing parts default to empty (pathname defaults to "/").
fn parse_url_parts(href: &str) -> (String, String, String, String, String) {
    let (proto, rest) = match href.find("://") {
        Some(i) => (href[..i + 1].to_string(), &href[i + 3..]),
        None => match href.find(':') {
            // Scheme-only URLs like "about:blank".
            Some(i) => (href[..i + 1].to_string(), &href[i + 1..]),
            None => (String::new(), href),
        },
    };
    // Split off hash then search.
    let (before_hash, hash) = match rest.find('#') {
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None => (rest, String::new()),
    };
    let (authority_path, search) = match before_hash.find('?') {
        Some(i) => (&before_hash[..i], before_hash[i..].to_string()),
        None => (before_hash, String::new()),
    };
    // Host is up to the first '/'; the rest is the path.
    let (host, pathname) = match authority_path.find('/') {
        Some(i) => (authority_path[..i].to_string(), authority_path[i..].to_string()),
        None => (authority_path.to_string(), "/".to_string()),
    };
    (proto, host, pathname, search, hash)
}

fn node_key(node: &Rc<RefCell<Node>>) -> usize {
    Rc::as_ptr(node) as usize
}

/// Record a DOM mutation so the runtime knows a re-layout is warranted.
fn bump_dom_generation() {
    DOM_GENERATION.with(|g| g.set(g.get().wrapping_add(1)));
}

/// Wrap a DOM node as an `Element` JsObject exposing live accessors
/// (textContent, id, className, tagName) and attribute methods.
///
/// Wrappers are cached by node identity so `el === el` holds across separate
/// lookups (plan D5). The cache pins the node's `Rc` (via the wrapper's
/// NodeHandle), so a cached node is never freed and its address never reused
/// while cached; the cache is cleared on navigation.
fn make_element(node: Rc<RefCell<Node>>, context: &mut Context) -> JsObject {
    let key = node_key(&node);
    if let Some(cached) = WRAPPER_CACHE.with(|c| c.borrow().get(&key).cloned()) {
        return cached;
    }
    let obj = build_element_wrapper(node, context);
    WRAPPER_CACHE.with(|c| c.borrow_mut().insert(key, obj.clone()));
    obj
}

fn build_element_wrapper(node: Rc<RefCell<Node>>, context: &mut Context) -> JsObject {
    let obj = JsObject::from_proto_and_data(None, NodeHandle { node });
    let realm = context.realm().clone();

    let accessor = |get: fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>,
                    set: Option<fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>>|
     -> PropertyDescriptor {
        let mut b = PropertyDescriptor::builder()
            .get(NativeFunction::from_fn_ptr(get).to_js_function(&realm))
            .enumerable(true)
            .configurable(true);
        if let Some(set) = set {
            b = b.set(NativeFunction::from_fn_ptr(set).to_js_function(&realm));
        }
        b.build()
    };

    obj.insert_property(
        js_string!("textContent"),
        accessor(element_get_text_content, Some(element_set_text_content)),
    );
    obj.insert_property(
        js_string!("id"),
        accessor(element_get_id, Some(element_set_id)),
    );
    obj.insert_property(
        js_string!("className"),
        accessor(element_get_class_name, Some(element_set_class_name)),
    );
    obj.insert_property(js_string!("tagName"), accessor(element_get_tag_name, None));
    obj.insert_property(
        js_string!("innerHTML"),
        accessor(element_get_inner_html, Some(element_set_inner_html)),
    );
    obj.insert_property(js_string!("classList"), accessor(element_get_class_list, None));
    obj.insert_property(js_string!("style"), accessor(element_get_style, None));
    obj.insert_property(js_string!("dataset"), accessor(element_get_dataset, None));
    obj.insert_property(js_string!("parentNode"), accessor(element_get_parent_node, None));
    obj.insert_property(
        js_string!("parentElement"),
        accessor(element_get_parent_node, None),
    );
    obj.insert_property(
        js_string!("firstChild"),
        accessor(element_get_first_child, None),
    );
    obj.insert_property(
        js_string!("lastChild"),
        accessor(element_get_last_child, None),
    );
    obj.insert_property(
        js_string!("nextSibling"),
        accessor(element_get_next_sibling, None),
    );
    obj.insert_property(
        js_string!("previousSibling"),
        accessor(element_get_previous_sibling, None),
    );
    obj.insert_property(
        js_string!("children"),
        accessor(element_get_children, None),
    );
    obj.insert_property(
        js_string!("childNodes"),
        accessor(element_get_child_nodes, None),
    );

    let method = |f: fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>, name, len| {
        let desc = PropertyDescriptor::builder()
            .value(NativeFunction::from_fn_ptr(f).to_js_function(&realm))
            .writable(true)
            .enumerable(false)
            .configurable(true)
            .build();
        obj.insert_property(name, desc);
        let _ = len;
    };
    method(element_get_attribute, js_string!("getAttribute"), 1);
    method(element_set_attribute, js_string!("setAttribute"), 2);
    method(element_has_attribute, js_string!("hasAttribute"), 1);
    method(element_append_child, js_string!("appendChild"), 1);
    method(element_remove_child, js_string!("removeChild"), 1);
    method(element_insert_before, js_string!("insertBefore"), 2);
    method(element_remove, js_string!("remove"), 0);
    method(elem_matches, js_string!("matches"), 1);
    method(elem_closest, js_string!("closest"), 1);
    method(elem_query_selector, js_string!("querySelector"), 1);
    method(elem_query_selector_all, js_string!("querySelectorAll"), 1);
    method(element_add_event_listener, js_string!("addEventListener"), 2);
    method(
        element_remove_event_listener,
        js_string!("removeEventListener"),
        2,
    );
    method(element_dispatch_event, js_string!("dispatchEvent"), 1);

    obj
}

/// Read an element attribute via the NodeHandle in `this`.
fn attr_of(this: &JsValue, name: &str) -> Option<String> {
    let node = handle_node(this)?;
    let b = node.borrow();
    match b.kind() {
        NodeKind::Element(ref e) => e.get_attribute(name),
        _ => None,
    }
}

/// Write an element attribute via the NodeHandle in `this`.
fn set_attr_of(this: &JsValue, name: &str, value: &str) {
    if let Some(node) = handle_node(this) {
        if let NodeKind::Element(ref mut e) = node.borrow_mut().kind_mut() {
            e.set_attribute(name, value);
        }
        bump_dom_generation();
    }
}

/// Unlink `child` from its current parent and siblings, if any.
fn detach_node(child: &Rc<RefCell<Node>>) {
    let parent = child.borrow().parent().upgrade();
    let prev = child.borrow().previous_sibling().upgrade();
    let next = child.borrow().next_sibling();

    match &prev {
        Some(p) => p.borrow_mut().set_next_sibling(next.clone()),
        None => {
            if let Some(par) = &parent {
                par.borrow_mut().set_first_child(next.clone());
            }
        }
    }
    match &next {
        Some(n) => n
            .borrow_mut()
            .set_previous_sibling(prev.as_ref().map(Rc::downgrade).unwrap_or_default()),
        None => {
            if let Some(par) = &parent {
                par.borrow_mut()
                    .set_last_child(prev.as_ref().map(Rc::downgrade).unwrap_or_default());
            }
        }
    }
    {
        let mut cb = child.borrow_mut();
        cb.set_parent(Weak::new());
        cb.set_previous_sibling(Weak::new());
        cb.set_next_sibling(None);
    }
    bump_dom_generation();
}

/// Append `child` as the last child of `parent` (detaching it first).
fn append_child_node(parent: &Rc<RefCell<Node>>, child: &Rc<RefCell<Node>>) {
    detach_node(child);
    let last = parent.borrow().last_child().upgrade();
    match &last {
        Some(l) => {
            l.borrow_mut().set_next_sibling(Some(child.clone()));
            child.borrow_mut().set_previous_sibling(Rc::downgrade(l));
        }
        None => {
            parent.borrow_mut().set_first_child(Some(child.clone()));
        }
    }
    parent.borrow_mut().set_last_child(Rc::downgrade(child));
    child.borrow_mut().set_parent(Rc::downgrade(parent));
    bump_dom_generation();
}

/// Insert `new_node` before `ref_node` under `parent`. If `ref_node` is None,
/// this is equivalent to appendChild.
fn insert_before_node(
    parent: &Rc<RefCell<Node>>,
    new_node: &Rc<RefCell<Node>>,
    ref_node: Option<&Rc<RefCell<Node>>>,
) {
    let Some(reference) = ref_node else {
        append_child_node(parent, new_node);
        return;
    };
    detach_node(new_node);
    let prev = reference.borrow().previous_sibling().upgrade();
    match &prev {
        Some(p) => p.borrow_mut().set_next_sibling(Some(new_node.clone())),
        None => parent.borrow_mut().set_first_child(Some(new_node.clone())),
    }
    {
        let mut nb = new_node.borrow_mut();
        nb.set_parent(Rc::downgrade(parent));
        nb.set_previous_sibling(prev.as_ref().map(Rc::downgrade).unwrap_or_default());
        nb.set_next_sibling(Some(reference.clone()));
    }
    reference
        .borrow_mut()
        .set_previous_sibling(Rc::downgrade(new_node));
    bump_dom_generation();
}

/// True if `ancestor` is a (transitive) parent of `node`.
fn is_child_of(ancestor: &Rc<RefCell<Node>>, node: &Rc<RefCell<Node>>) -> bool {
    node.borrow()
        .parent()
        .upgrade()
        .map(|p| Rc::ptr_eq(&p, ancestor))
        .unwrap_or(false)
}

fn element_append_child(this: &JsValue, a: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let (Some(parent), Some(child)) = (handle_node(this), a.first().and_then(handle_node)) else {
        return Ok(JsValue::null());
    };
    append_child_node(&parent, &child);
    Ok(JsValue::from(make_element(child, ctx)))
}

fn element_remove_child(this: &JsValue, a: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let (Some(parent), Some(child)) = (handle_node(this), a.first().and_then(handle_node)) else {
        return Ok(JsValue::null());
    };
    if !is_child_of(&parent, &child) {
        return Ok(JsValue::null());
    }
    detach_node(&child);
    Ok(JsValue::from(make_element(child, ctx)))
}

fn element_insert_before(this: &JsValue, a: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let (Some(parent), Some(new_node)) = (handle_node(this), a.first().and_then(handle_node)) else {
        return Ok(JsValue::null());
    };
    let reference = a.get(1).and_then(handle_node);
    insert_before_node(&parent, &new_node, reference.as_ref());
    Ok(JsValue::from(make_element(new_node, ctx)))
}

/// `element.remove()` — detach the element from its parent.
fn element_remove(this: &JsValue, _a: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    if let Some(node) = handle_node(this) {
        detach_node(&node);
    }
    Ok(JsValue::undefined())
}

fn element_add_event_listener(
    this: &JsValue,
    a: &[JsValue],
    c: &mut Context,
) -> JsResult<JsValue> {
    let Some(node) = handle_node(this) else {
        return Ok(JsValue::undefined());
    };
    let event_type = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    let Some(cb) = a.get(1).and_then(|v| v.as_object()).cloned() else {
        return Ok(JsValue::undefined());
    };
    let capture = parse_capture(a.get(2), c);
    LISTENERS.with(|m| {
        m.borrow_mut()
            .entry(node_key(&node))
            .or_default()
            .push(Listener { event_type, callback: cb, capture });
    });
    Ok(JsValue::undefined())
}

fn element_remove_event_listener(
    this: &JsValue,
    a: &[JsValue],
    c: &mut Context,
) -> JsResult<JsValue> {
    let Some(node) = handle_node(this) else {
        return Ok(JsValue::undefined());
    };
    let event_type = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    let cb = a.get(1).and_then(|v| v.as_object()).cloned();
    LISTENERS.with(|m| {
        if let Some(v) = m.borrow_mut().get_mut(&node_key(&node)) {
            v.retain(|l| {
                l.event_type != event_type || cb.as_ref().map(|c| c != &l.callback).unwrap_or(true)
            });
        }
    });
    Ok(JsValue::undefined())
}

fn element_dispatch_event(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let Some(node) = handle_node(this) else {
        return Ok(JsValue::from(true));
    };
    // Accept either an Event-like object with a `type` property or a bare
    // string naming the event type.
    let event_type = match a.first() {
        Some(v) if v.is_object() => v
            .as_object()
            .unwrap()
            .get(js_string!("type"), c)?
            .to_string(c)?
            .to_std_string_escaped(),
        Some(v) => v.clone().to_string(c)?.to_std_string_escaped(),
        None => String::new(),
    };
    let not_prevented = run_dispatch(node, &event_type, None, c);
    Ok(JsValue::from(not_prevented))
}

/// Build an `Event` JsObject carrying `type`, `target`, propagation flags, and
/// the `preventDefault` / `stopPropagation` methods.
/// Build the `Event` object handed to listeners. `mouse` carries the pointer
/// position for mouse events (`click` etc.), which also gain the button and
/// modifier-key fields scripts routinely gate on — a missing `button` reads as
/// `undefined`, and the common `if (event.button !== 0) return;` guard would
/// then reject every synthesized click.
/// Spec: UI Events — MouseEvent. https://w3c.github.io/uievents/#mouseevent
fn make_event(
    target: &Rc<RefCell<Node>>,
    event_type: &str,
    mouse: Option<(f64, f64)>,
    ctx: &mut Context,
) -> JsObject {
    let obj = JsObject::from_proto_and_data(
        None,
        EventFlags {
            default_prevented: std::cell::Cell::new(false),
            stop_propagation: std::cell::Cell::new(false),
        },
    );
    let realm = ctx.realm().clone();
    obj.insert_property(
        js_string!("type"),
        PropertyDescriptor::builder()
            .value(js_string!(event_type))
            .writable(false)
            .enumerable(true)
            .configurable(true)
            .build(),
    );
    let target_val = JsValue::from(make_element(target.clone(), ctx));
    obj.insert_property(
        js_string!("target"),
        PropertyDescriptor::builder()
            .value(target_val)
            .writable(false)
            .enumerable(true)
            .configurable(true)
            .build(),
    );
    let method = |f: fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>, name| {
        obj.insert_property(
            name,
            PropertyDescriptor::builder()
                .value(NativeFunction::from_fn_ptr(f).to_js_function(&realm))
                .writable(true)
                .enumerable(false)
                .configurable(true)
                .build(),
        );
    };
    method(event_prevent_default, js_string!("preventDefault"));
    method(event_stop_propagation, js_string!("stopPropagation"));

    let constant = |name, value: JsValue| {
        obj.insert_property(
            name,
            PropertyDescriptor::builder()
                .value(value)
                .writable(false)
                .enumerable(true)
                .configurable(true)
                .build(),
        );
    };
    constant(js_string!("bubbles"), JsValue::from(true));
    constant(js_string!("cancelable"), JsValue::from(true));
    if let Some((x, y)) = mouse {
        // Primary (left) button, no modifiers — the only kind of click the GUI
        // synthesizes today.
        constant(js_string!("button"), JsValue::from(0));
        constant(js_string!("buttons"), JsValue::from(1));
        for key in ["altKey", "ctrlKey", "metaKey", "shiftKey"] {
            constant(js_string!(key), JsValue::from(false));
        }
        for key in ["clientX", "pageX", "x", "offsetX"] {
            constant(js_string!(key), JsValue::from(x));
        }
        for key in ["clientY", "pageY", "y", "offsetY"] {
            constant(js_string!(key), JsValue::from(y));
        }
    }
    // `defaultPrevented` reflects the flag preventDefault() sets.
    obj.insert_property(
        js_string!("defaultPrevented"),
        PropertyDescriptor::builder()
            .get(
                NativeFunction::from_fn_ptr(event_default_prevented)
                    .to_js_function(&realm),
            )
            .enumerable(true)
            .configurable(true)
            .build(),
    );
    obj
}

fn event_default_prevented(this: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    let prevented = this
        .as_object()
        .and_then(|obj| obj.downcast_ref::<EventFlags>().map(|f| f.default_prevented.get()))
        .unwrap_or(false);
    Ok(JsValue::from(prevented))
}

fn event_prevent_default(this: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    if let Some(obj) = this.as_object() {
        if let Some(f) = obj.downcast_ref::<EventFlags>() {
            f.default_prevented.set(true);
        }
    }
    Ok(JsValue::undefined())
}

fn event_stop_propagation(this: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    if let Some(obj) = this.as_object() {
        if let Some(f) = obj.downcast_ref::<EventFlags>() {
            f.stop_propagation.set(true);
        }
    }
    Ok(JsValue::undefined())
}

/// Dispatch `event_type` at `target` through the standard three phases:
/// capture (root→target's parent), at-target (both), then bubble
/// (parent→root). Returns `true` if `preventDefault` was NOT called (i.e. the
/// default action runs).
fn run_dispatch(
    target: Rc<RefCell<Node>>,
    event_type: &str,
    mouse: Option<(f64, f64)>,
    ctx: &mut Context,
) -> bool {
    let event = make_event(&target, event_type, mouse, ctx);

    // Propagation path: [target, parent, ..., root].
    let mut path = vec![target.clone()];
    let mut cur = target.borrow().parent().upgrade();
    while let Some(p) = cur {
        path.push(p.clone());
        cur = p.borrow().parent().upgrade();
    }

    let stopped = |event: &JsObject| {
        event
            .downcast_ref::<EventFlags>()
            .map(|f| f.stop_propagation.get())
            .unwrap_or(false)
    };

    // Fire listeners on `node` whose capture flag is in `want`. `None` means
    // "either" (the at-target phase runs both capture and bubble listeners).
    // Returns false if propagation was stopped.
    fn fire(
        node: &Rc<RefCell<Node>>,
        event_type: &str,
        want: Option<bool>,
        event: &JsObject,
        stopped: &dyn Fn(&JsObject) -> bool,
        ctx: &mut Context,
    ) -> bool {
        let callbacks: Vec<JsObject> = LISTENERS.with(|m| {
            m.borrow()
                .get(&node_key(node))
                .map(|v| {
                    v.iter()
                        .filter(|l| {
                            l.event_type == event_type
                                && want.map(|w| l.capture == w).unwrap_or(true)
                        })
                        .map(|l| l.callback.clone())
                        .collect()
                })
                .unwrap_or_default()
        });
        for cb in callbacks {
            let this = JsValue::from(make_element(node.clone(), ctx));
            let _ = cb.call(&this, &[JsValue::from(event.clone())], ctx);
            if stopped(event) {
                return false;
            }
        }
        true
    }

    // Capture phase: root down to (but not including) the target.
    for node in path[1..].iter().rev() {
        if !fire(node, event_type, Some(true), &event, &stopped, ctx) {
            return default_allowed(&event);
        }
    }
    // At target: both capture and bubble listeners.
    if !fire(&target, event_type, None, &event, &stopped, ctx) {
        return default_allowed(&event);
    }
    // Bubble phase: parent up to root.
    for node in path[1..].iter() {
        if !fire(node, event_type, Some(false), &event, &stopped, ctx) {
            return default_allowed(&event);
        }
    }

    default_allowed(&event)
}

fn default_allowed(event: &JsObject) -> bool {
    !event
        .downcast_ref::<EventFlags>()
        .map(|f| f.default_prevented.get())
        .unwrap_or(false)
}

fn node_or_null(node: Option<Rc<RefCell<Node>>>, ctx: &mut Context) -> JsValue {
    match node {
        Some(n) => JsValue::from(make_element(n, ctx)),
        None => JsValue::null(),
    }
}

fn elem_matches(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let Some(node) = handle_node(this) else {
        return Ok(JsValue::from(false));
    };
    let sel = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    Ok(JsValue::from(element_matches(&node, &sel)))
}

fn elem_closest(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let Some(node) = handle_node(this) else {
        return Ok(JsValue::null());
    };
    let sel = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    Ok(node_or_null(element_closest(node, &sel), c))
}

/// Element-scoped querySelector: matches within `this`'s subtree.
fn elem_query_selector(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let Some(node) = handle_node(this) else {
        return Ok(JsValue::null());
    };
    let sel = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    // Element-scoped query matches descendants only, never the element itself.
    let found = query_selector_all(node.clone(), &sel)
        .into_iter()
        .find(|n| !Rc::ptr_eq(n, &node));
    Ok(node_or_null(found, c))
}

fn elem_query_selector_all(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let Some(node) = handle_node(this) else {
        return Ok(JsValue::from(JsArray::new(c)));
    };
    let sel = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    let arr = JsArray::new(c);
    // Descendants only — exclude the element itself if it happens to match.
    for n in query_selector_all(node.clone(), &sel) {
        if Rc::ptr_eq(&n, &node) {
            continue;
        }
        let el = make_element(n, c);
        arr.push(JsValue::from(el), c)?;
    }
    Ok(JsValue::from(arr))
}

fn element_get_parent_node(this: &JsValue, _a: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let parent = handle_node(this).and_then(|n| n.borrow().parent().upgrade());
    Ok(node_or_null(parent, ctx))
}
fn element_get_first_child(this: &JsValue, _a: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let n = handle_node(this).and_then(|n| n.borrow().first_child());
    Ok(node_or_null(n, ctx))
}
fn element_get_last_child(this: &JsValue, _a: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let n = handle_node(this).and_then(|n| n.borrow().last_child().upgrade());
    Ok(node_or_null(n, ctx))
}
fn element_get_next_sibling(this: &JsValue, _a: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let n = handle_node(this).and_then(|n| n.borrow().next_sibling());
    Ok(node_or_null(n, ctx))
}
fn element_get_previous_sibling(
    this: &JsValue,
    _a: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let n = handle_node(this).and_then(|n| n.borrow().previous_sibling().upgrade());
    Ok(node_or_null(n, ctx))
}

/// Collect the direct children of `this`, optionally elements only.
fn child_nodes(this: &JsValue, elements_only: bool) -> Vec<Rc<RefCell<Node>>> {
    let Some(node) = handle_node(this) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cur = node.borrow().first_child();
    while let Some(c) = cur {
        let is_element = matches!(c.borrow().kind(), NodeKind::Element(_));
        if !elements_only || is_element {
            out.push(c.clone());
        }
        cur = c.borrow().next_sibling();
    }
    out
}

fn element_get_children(this: &JsValue, _a: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let arr = JsArray::new(ctx);
    for c in child_nodes(this, true) {
        let el = make_element(c, ctx);
        arr.push(JsValue::from(el), ctx)?;
    }
    Ok(JsValue::from(arr))
}
fn element_get_child_nodes(this: &JsValue, _a: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let arr = JsArray::new(ctx);
    for c in child_nodes(this, false) {
        let el = make_element(c, ctx);
        arr.push(JsValue::from(el), ctx)?;
    }
    Ok(JsValue::from(arr))
}

fn element_get_id(this: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(js_string!(attr_of(this, "id")
        .unwrap_or_default()
        .as_str())))
}
fn element_set_id(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let v = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    set_attr_of(this, "id", &v);
    Ok(JsValue::undefined())
}
fn element_get_class_name(this: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(js_string!(attr_of(this, "class")
        .unwrap_or_default()
        .as_str())))
}
fn element_set_class_name(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let v = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    set_attr_of(this, "class", &v);
    Ok(JsValue::undefined())
}
fn element_get_tag_name(this: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    let tag = handle_node(this)
        .and_then(|n| match n.borrow().kind() {
            NodeKind::Element(ref e) => Some(e.tag_name().to_uppercase()),
            _ => None,
        })
        .unwrap_or_default();
    Ok(JsValue::from(js_string!(tag.as_str())))
}
fn element_get_attribute(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let name = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    Ok(match attr_of(this, &name) {
        Some(v) => JsValue::from(js_string!(v.as_str())),
        None => JsValue::null(),
    })
}
fn element_set_attribute(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let name = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    let value = a.get(1).cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    set_attr_of(this, &name, &value);
    Ok(JsValue::undefined())
}
fn element_has_attribute(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let name = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    Ok(JsValue::from(attr_of(this, &name).is_some()))
}

/// `element.classList` — a live token-list view over the `class` attribute.
/// The returned object carries the same NodeHandle, so its methods mutate the
/// element's class attribute directly.
fn element_get_class_list(this: &JsValue, _a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let Some(node) = handle_node(this) else {
        return Ok(JsValue::undefined());
    };
    let obj = JsObject::from_proto_and_data(None, NodeHandle { node });
    let realm = c.realm().clone();
    let method = |f: fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>, name| {
        let desc = PropertyDescriptor::builder()
            .value(NativeFunction::from_fn_ptr(f).to_js_function(&realm))
            .writable(true)
            .enumerable(false)
            .configurable(true)
            .build();
        obj.insert_property(name, desc);
    };
    method(classlist_add, js_string!("add"));
    method(classlist_remove, js_string!("remove"));
    method(classlist_toggle, js_string!("toggle"));
    method(classlist_contains, js_string!("contains"));
    Ok(JsValue::from(obj))
}

/// camelCase → kebab-case CSS property names exposed as `style.<prop>`
/// accessors. `setProperty`/`getPropertyValue` cover anything not listed.
const STYLE_PROPS: &[(&str, &str)] = &[
    ("color", "color"),
    ("backgroundColor", "background-color"),
    ("background", "background"),
    ("display", "display"),
    ("visibility", "visibility"),
    ("opacity", "opacity"),
    ("width", "width"),
    ("height", "height"),
    ("minWidth", "min-width"),
    ("maxWidth", "max-width"),
    ("minHeight", "min-height"),
    ("maxHeight", "max-height"),
    ("top", "top"),
    ("left", "left"),
    ("right", "right"),
    ("bottom", "bottom"),
    ("position", "position"),
    ("margin", "margin"),
    ("padding", "padding"),
    ("border", "border"),
    ("borderColor", "border-color"),
    ("borderRadius", "border-radius"),
    ("fontSize", "font-size"),
    ("fontWeight", "font-weight"),
    ("fontFamily", "font-family"),
    ("textAlign", "text-align"),
    ("lineHeight", "line-height"),
    ("zIndex", "z-index"),
    ("overflow", "overflow"),
    ("flex", "flex"),
    ("flexDirection", "flex-direction"),
    ("justifyContent", "justify-content"),
    ("alignItems", "align-items"),
    ("gap", "gap"),
    ("transform", "transform"),
    ("transition", "transition"),
    ("cursor", "cursor"),
];

/// Parse a `style="..."` attribute value into ordered (property, value) pairs.
fn parse_style_decls(css: &str) -> Vec<(String, String)> {
    css.split(';')
        .filter_map(|decl| {
            let decl = decl.trim();
            if decl.is_empty() {
                return None;
            }
            let (prop, value) = decl.split_once(':')?;
            Some((prop.trim().to_ascii_lowercase(), value.trim().to_string()))
        })
        .collect()
}

fn serialize_style_decls(decls: &[(String, String)]) -> String {
    decls
        .iter()
        .map(|(p, v)| format!("{p}: {v}"))
        .collect::<Vec<_>>()
        .join("; ")
}

fn style_get(this: &JsValue, prop: &str) -> Option<String> {
    let prop = prop.to_ascii_lowercase();
    parse_style_decls(&attr_of(this, "style").unwrap_or_default())
        .into_iter()
        .find(|(p, _)| *p == prop)
        .map(|(_, v)| v)
}

fn style_set(this: &JsValue, prop: &str, value: &str) {
    let prop = prop.to_ascii_lowercase();
    let mut decls = parse_style_decls(&attr_of(this, "style").unwrap_or_default());
    // An empty value removes the declaration (matches setProperty('', '')).
    if value.trim().is_empty() {
        decls.retain(|(p, _)| *p != prop);
    } else if let Some(entry) = decls.iter_mut().find(|(p, _)| *p == prop) {
        entry.1 = value.trim().to_string();
    } else {
        decls.push((prop, value.trim().to_string()));
    }
    set_attr_of(this, "style", &serialize_style_decls(&decls));
}

fn style_remove(this: &JsValue, prop: &str) {
    let prop = prop.to_ascii_lowercase();
    let mut decls = parse_style_decls(&attr_of(this, "style").unwrap_or_default());
    decls.retain(|(p, _)| *p != prop);
    set_attr_of(this, "style", &serialize_style_decls(&decls));
}

/// `element.style` — a live view over the inline `style` attribute. Exposes
/// setProperty/getPropertyValue/removeProperty + cssText and camelCase
/// accessors for common properties. The engine re-parses the style attribute
/// at layout time, so writes here take effect on the next relayout.
fn element_get_style(this: &JsValue, _a: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let Some(node) = handle_node(this) else {
        return Ok(JsValue::undefined());
    };
    let obj = JsObject::from_proto_and_data(None, NodeHandle { node });
    let realm = ctx.realm().clone();

    let method = |f: fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>, name| {
        obj.insert_property(
            name,
            PropertyDescriptor::builder()
                .value(NativeFunction::from_fn_ptr(f).to_js_function(&realm))
                .writable(true)
                .enumerable(false)
                .configurable(true)
                .build(),
        );
    };
    method(style_set_property, js_string!("setProperty"));
    method(style_get_property_value, js_string!("getPropertyValue"));
    method(style_remove_property, js_string!("removeProperty"));
    obj.insert_property(
        js_string!("cssText"),
        PropertyDescriptor::builder()
            .get(NativeFunction::from_fn_ptr(style_get_css_text).to_js_function(&realm))
            .set(NativeFunction::from_fn_ptr(style_set_css_text).to_js_function(&realm))
            .enumerable(true)
            .configurable(true)
            .build(),
    );

    // camelCase accessors that map to individual CSS properties. Closures
    // capture the (Copy) &'static kebab name.
    for (js_name, css_name) in STYLE_PROPS {
        let css_for_get: &'static str = css_name;
        let getter = NativeFunction::from_copy_closure(move |this: &JsValue, _a: &[JsValue], _c: &mut Context| {
            Ok(JsValue::from(js_string!(style_get(this, css_for_get)
                .unwrap_or_default()
                .as_str())))
        })
        .to_js_function(&realm);
        let css_for_set: &'static str = css_name;
        let setter = NativeFunction::from_copy_closure(move |this: &JsValue, a: &[JsValue], c: &mut Context| {
            let v = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
            style_set(this, css_for_set, &v);
            Ok(JsValue::undefined())
        })
        .to_js_function(&realm);
        obj.insert_property(
            js_string!(*js_name),
            PropertyDescriptor::builder()
                .get(getter)
                .set(setter)
                .enumerable(true)
                .configurable(true)
                .build(),
        );
    }

    Ok(JsValue::from(obj))
}

fn style_set_property(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let name = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    let value = a.get(1).cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    style_set(this, &name, &value);
    Ok(JsValue::undefined())
}
fn style_get_property_value(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let name = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    Ok(JsValue::from(js_string!(style_get(this, &name).unwrap_or_default().as_str())))
}
fn style_remove_property(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let name = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    let prev = style_get(this, &name).unwrap_or_default();
    style_remove(this, &name);
    Ok(JsValue::from(js_string!(prev.as_str())))
}
fn style_get_css_text(this: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(js_string!(attr_of(this, "style").unwrap_or_default().as_str())))
}
fn style_set_css_text(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let text = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    // Normalize through the parser so subsequent reads are consistent.
    set_attr_of(this, "style", &serialize_style_decls(&parse_style_decls(&text)));
    Ok(JsValue::undefined())
}

/// `data-foo-bar` attribute name → `fooBar` dataset key.
fn data_attr_to_key(attr: &str) -> Option<String> {
    let rest = attr.strip_prefix("data-")?;
    let mut out = String::new();
    let mut upper = false;
    for c in rest.chars() {
        if c == '-' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// `fooBar` dataset key → `data-foo-bar` attribute name.
fn key_to_data_attr(key: &str) -> String {
    let mut out = String::from("data-");
    for c in key.chars() {
        if c.is_ascii_uppercase() {
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// `element.dataset` — a Proxy over the element's `data-*` attributes with
/// camelCase keys (get/set/has/delete/enumerate). The proxy target carries the
/// element's NodeHandle so the fn-pointer traps can reach the node via arg 0.
fn element_get_dataset(this: &JsValue, _a: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let Some(node) = handle_node(this) else {
        return Ok(JsValue::undefined());
    };
    let target = JsObject::from_proto_and_data(None, NodeHandle { node });
    let proxy = JsProxy::builder(target)
        .get(dataset_get)
        .set(dataset_set)
        .has(dataset_has)
        .delete_property(dataset_delete)
        .own_keys(dataset_own_keys)
        .get_own_property_descriptor(dataset_own_desc)
        .build(ctx);
    Ok(JsValue::from(proxy))
}

fn dataset_get(_this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let target = a.first().cloned().unwrap_or_default();
    let key = a.get(1).cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    Ok(match attr_of(&target, &key_to_data_attr(&key)) {
        Some(v) => JsValue::from(js_string!(v.as_str())),
        None => JsValue::undefined(),
    })
}

fn dataset_set(_this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let target = a.first().cloned().unwrap_or_default();
    let key = a.get(1).cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    let value = a.get(2).cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    set_attr_of(&target, &key_to_data_attr(&key), &value);
    Ok(JsValue::from(true))
}

fn dataset_has(_this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let target = a.first().cloned().unwrap_or_default();
    let key = a.get(1).cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    Ok(JsValue::from(attr_of(&target, &key_to_data_attr(&key)).is_some()))
}

fn dataset_delete(_this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let target = a.first().cloned().unwrap_or_default();
    let key = a.get(1).cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    if let Some(node) = handle_node(&target) {
        if let NodeKind::Element(ref mut e) = node.borrow_mut().kind_mut() {
            e.remove_attribute(&key_to_data_attr(&key));
        }
        bump_dom_generation();
    }
    Ok(JsValue::from(true))
}

/// Collect the dataset's camelCase keys from the element's data-* attributes.
fn dataset_keys(target: &JsValue) -> Vec<String> {
    let Some(node) = handle_node(target) else {
        return Vec::new();
    };
    let attrs = match node.borrow().kind() {
        NodeKind::Element(e) => e.attributes(),
        _ => return Vec::new(),
    };
    attrs
        .iter()
        .filter_map(|a| data_attr_to_key(&a.name()))
        .collect()
}

fn dataset_own_keys(_this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let target = a.first().cloned().unwrap_or_default();
    let arr = JsArray::new(c);
    for key in dataset_keys(&target) {
        arr.push(JsValue::from(js_string!(key.as_str())), c)?;
    }
    Ok(JsValue::from(arr))
}

/// getOwnPropertyDescriptor trap — required for Object.keys / enumeration to
/// surface the dataset keys. Returns an enumerable, configurable data prop.
fn dataset_own_desc(_this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let target = a.first().cloned().unwrap_or_default();
    let key = a.get(1).cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    match attr_of(&target, &key_to_data_attr(&key)) {
        Some(v) => {
            let desc = ObjectInitializer::new(c)
                .property(js_string!("value"), js_string!(v.as_str()), Attribute::all())
                .property(js_string!("writable"), true, Attribute::all())
                .property(js_string!("enumerable"), true, Attribute::all())
                .property(js_string!("configurable"), true, Attribute::all())
                .build();
            Ok(JsValue::from(desc))
        }
        None => Ok(JsValue::undefined()),
    }
}

fn class_tokens(this: &JsValue) -> Vec<String> {
    attr_of(this, "class")
        .unwrap_or_default()
        .split_whitespace()
        .map(|s| s.to_string())
        .collect()
}

fn set_class_tokens(this: &JsValue, tokens: &[String]) {
    set_attr_of(this, "class", &tokens.join(" "));
}

fn classlist_add(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let mut tokens = class_tokens(this);
    for arg in a {
        let t = arg.clone().to_string(c)?.to_std_string_escaped();
        if !t.is_empty() && !tokens.iter().any(|x| x == &t) {
            tokens.push(t);
        }
    }
    set_class_tokens(this, &tokens);
    Ok(JsValue::undefined())
}

fn classlist_remove(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let mut tokens = class_tokens(this);
    let mut drop: Vec<String> = Vec::new();
    for arg in a {
        drop.push(arg.clone().to_string(c)?.to_std_string_escaped());
    }
    tokens.retain(|x| !drop.iter().any(|d| d == x));
    set_class_tokens(this, &tokens);
    Ok(JsValue::undefined())
}

fn classlist_toggle(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let token = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    let mut tokens = class_tokens(this);
    let present = tokens.iter().any(|x| x == &token);
    // Optional second argument forces the resulting state.
    let force = a.get(1).map(|v| v.to_boolean());
    let should_have = force.unwrap_or(!present);
    if should_have {
        if !present && !token.is_empty() {
            tokens.push(token);
        }
    } else {
        tokens.retain(|x| x != &token);
    }
    set_class_tokens(this, &tokens);
    Ok(JsValue::from(should_have))
}

fn classlist_contains(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let token = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    Ok(JsValue::from(class_tokens(this).iter().any(|x| x == &token)))
}

/// Void elements never emit a closing tag when serialized.
fn is_void_element(tag: &str) -> bool {
    matches!(
        tag,
        "area" | "base" | "br" | "col" | "embed" | "hr" | "img" | "input" | "link" | "meta"
            | "param" | "source" | "track" | "wbr"
    )
}

/// Serialize a node's children to an HTML string (the `innerHTML` getter).
fn serialize_children(node: &Rc<RefCell<Node>>, out: &mut String) {
    let mut cur = node.borrow().first_child();
    while let Some(c) = cur {
        serialize_node(&c, out);
        cur = c.borrow().next_sibling();
    }
}

/// Escape a text node's data for HTML serialization (`&`, `<`, `>`).
fn escape_html_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Escape an attribute value for the double-quoted form (`&`, `"`).
fn escape_html_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;")
}

fn serialize_node(node: &Rc<RefCell<Node>>, out: &mut String) {
    match node.borrow().kind() {
        NodeKind::Text(t) => out.push_str(&escape_html_text(&t)),
        NodeKind::Element(e) => {
            let tag = e.tag_name().to_string();
            out.push('<');
            out.push_str(&tag);
            for attr in e.attributes() {
                out.push(' ');
                out.push_str(&attr.name());
                out.push_str("=\"");
                out.push_str(&escape_html_attr(&attr.value()));
                out.push('"');
            }
            out.push('>');
            if !is_void_element(&tag) {
                serialize_children(node, out);
                out.push_str("</");
                out.push_str(&tag);
                out.push('>');
            }
        }
        _ => {}
    }
}

fn element_get_inner_html(this: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    let Some(node) = handle_node(this) else {
        return Ok(JsValue::from(js_string!("")));
    };
    let mut s = String::new();
    serialize_children(&node, &mut s);
    Ok(JsValue::from(js_string!(s.as_str())))
}

/// `innerHTML` setter: parse the markup as a fragment and replace all of the
/// element's children with the parsed nodes. The fragment is parsed as a full
/// document; the resulting <body>'s children are re-parented onto the target.
fn element_set_inner_html(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let Some(target) = handle_node(this) else {
        return Ok(JsValue::undefined());
    };
    let html = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();

    // Remove the target's existing children.
    {
        let mut cur = target.borrow().first_child();
        while let Some(child) = cur {
            let next = child.borrow().next_sibling();
            child.borrow_mut().set_parent(Weak::new());
            child.borrow_mut().set_previous_sibling(Weak::new());
            child.borrow_mut().set_next_sibling(None);
            cur = next;
        }
        target.borrow_mut().set_first_child(None);
        target.borrow_mut().set_last_child(Weak::new());
    }

    // Parse the fragment and re-parent the body's children under the target.
    let window = HtmlParser::new(HtmlTokenizer::new(html)).construct_tree();
    let doc = window.borrow().document();
    if let Some(body) = get_target_element_node(Some(doc), ElementKind::Body) {
        let mut new_children = Vec::new();
        let mut cur = body.borrow().first_child();
        while let Some(child) = cur {
            new_children.push(child.clone());
            cur = child.borrow().next_sibling();
        }
        for child in new_children {
            append_child_node(&target, &child);
        }
    }
    // Covers the case of clearing to empty (no appends above).
    bump_dom_generation();
    Ok(JsValue::undefined())
}

fn handle_node(this: &JsValue) -> Option<Rc<RefCell<Node>>> {
    this.as_object()
        .and_then(|o| o.downcast_ref::<NodeHandle>().map(|h| h.node.clone()))
}

fn element_get_text_content(
    this: &JsValue,
    _args: &[JsValue],
    _ctx: &mut Context,
) -> JsResult<JsValue> {
    let Some(node) = handle_node(this) else {
        return Ok(JsValue::undefined());
    };
    // A text node's textContent is its own data; an element's is the
    // concatenation of its descendants' text.
    if let NodeKind::Text(ref t) = node.borrow().kind() {
        return Ok(JsValue::from(js_string!(t.as_str())));
    }
    let mut s = String::new();
    collect_text(node.borrow().first_child(), &mut s);
    Ok(JsValue::from(js_string!(s.as_str())))
}

fn element_set_text_content(
    this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let Some(node) = handle_node(this) else {
        return Ok(JsValue::undefined());
    };
    let text = args
        .first()
        .cloned()
        .unwrap_or(JsValue::undefined())
        .to_string(ctx)?
        .to_std_string_escaped();
    // On a text node, replace its data in place; on an element, replace all
    // children with a single text node.
    if let NodeKind::Text(_) = node.borrow().kind() {
        *node.borrow_mut().kind_mut() = NodeKind::Text(text);
        bump_dom_generation();
        return Ok(JsValue::undefined());
    }
    node.borrow_mut()
        .set_first_child(Some(Rc::new(RefCell::new(Node::new(NodeKind::Text(text))))));
    bump_dom_generation();
    Ok(JsValue::undefined())
}


/// Default per-loop iteration cap. Above realistic page-load loops but bounds
/// a runaway `while(true)` to a few seconds (Boa is an interpreter; a
/// thread-based timeout isn't possible because its Context is !Send).
const DEFAULT_LOOP_ITERATION_LIMIT: u64 = 5_000_000;

/// A per-page JavaScript execution context.
pub struct ScriptHost {
    context: Context,
    /// This host's own per-page state (plan D5). Made the active page before
    /// any JS runs so native functions see it.
    state: Rc<PageState>,
}

impl Default for ScriptHost {
    fn default() -> Self {
        Self::new()
    }
}

impl ScriptHost {
    pub fn new() -> Self {
        let mut host = Self {
            context: Context::default(),
            state: Rc::new(PageState::default()),
        };
        host.activate();
        // Watchdog: bound execution so a runaway loop or deep recursion throws
        // a catchable error instead of hanging the render thread (Boa's Context
        // is !Send, so a thread-based timeout isn't possible). These are per
        // single loop / recursion depth, generously above legitimate page code.
        {
            let limits = host.context.runtime_limits_mut();
            limits.set_loop_iteration_limit(DEFAULT_LOOP_ITERATION_LIMIT);
            limits.set_recursion_limit(2_000);
        }
        host.install_dom_globals();
        host
    }

    /// Point the `ACTIVE_PAGE` thread-local at this host's state so native
    /// functions operate on it. Called at the start of every public entry
    /// point that runs JS or touches page state.
    fn activate(&self) {
        ACTIVE_PAGE.with(|p| *p.borrow_mut() = Some(self.state.clone()));
    }

    /// Override the per-loop iteration cap (watchdog). Mainly for tests that
    /// want a runaway loop to fail fast without waiting for the default cap.
    pub fn set_loop_iteration_limit(&mut self, n: u64) {
        self.context.runtime_limits_mut().set_loop_iteration_limit(n);
    }

    /// Expose the given document root to script as `document`. Resets all
    /// per-page state (listeners, timers, wrapper cache, console/message
    /// buffers) left over from a previous document — plan D5: these registries
    /// are per-page and reset on navigation. `localStorage` intentionally
    /// persists (per-origin; seed it via `set_local_storage_entries`).
    pub fn set_document(&mut self, root: Rc<RefCell<Node>>) {
        self.activate();
        LISTENERS.with(|m| m.borrow_mut().clear());
        TIMERS.with(|t| t.borrow_mut().clear());
        WRAPPER_CACHE.with(|c| c.borrow_mut().clear());
        CONSOLE_LOG.with(|l| l.borrow_mut().clear());
        POSTED_MESSAGES.with(|m| m.borrow_mut().clear());
        PENDING_FETCHES.with(|p| p.borrow_mut().clear());
        PENDING_XHR.with(|p| p.borrow_mut().clear());
        VIRTUAL_CLOCK.with(|c| c.set(0));
        DOM_GENERATION.with(|g| g.set(0));
        SCRIPT_DOM.with(|d| *d.borrow_mut() = Some(root));
    }

    /// Fire an event of `event_type` at `target`, bubbling up its ancestor
    /// chain. Returns `true` if the default action should run (i.e.
    /// `preventDefault` was not called). Used by the runtime to route real
    /// input events (click/input/submit) into script.
    pub fn dispatch_event(&mut self, target: Rc<RefCell<Node>>, event_type: &str) -> bool {
        self.activate();
        run_dispatch(target, event_type, None, &mut self.context)
    }

    /// As [`dispatch_event`], for a pointer event at document coordinates
    /// `(x, y)`: the event additionally carries `button`/modifier keys and the
    /// coordinate fields (`clientX`/`pageX`/…) that click handlers read.
    pub fn dispatch_mouse_event(
        &mut self,
        target: Rc<RefCell<Node>>,
        event_type: &str,
        x: f64,
        y: f64,
    ) -> bool {
        self.activate();
        run_dispatch(target, event_type, Some((x, y)), &mut self.context)
    }

    fn install_dom_globals(&mut self) {
        // document.getElementById(id) — for now returns the element's
        // textContent string (or null). A real Element wrapper with
        // mutable textContent / DOM methods is the next step (plan 3.2).
        let document = ObjectInitializer::new(&mut self.context)
            .function(
                NativeFunction::from_fn_ptr(dom_get_element_by_id),
                js_string!("getElementById"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(dom_query_selector),
                js_string!("querySelector"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(dom_query_selector_all),
                js_string!("querySelectorAll"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(dom_create_element),
                js_string!("createElement"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(dom_create_text_node),
                js_string!("createTextNode"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(dom_get_elements_by_tag_name),
                js_string!("getElementsByTagName"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(dom_get_elements_by_class_name),
                js_string!("getElementsByClassName"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(doc_add_event_listener),
                js_string!("addEventListener"),
                2,
            )
            .function(
                NativeFunction::from_fn_ptr(doc_remove_event_listener),
                js_string!("removeEventListener"),
                2,
            )
            .function(
                NativeFunction::from_fn_ptr(doc_dispatch_event),
                js_string!("dispatchEvent"),
                1,
            )
            .build();
        self.context
            .register_global_property(js_string!("document"), document, Attribute::all())
            .expect("register document");

        // console.{log,info,warn,error,debug}
        let console = ObjectInitializer::new(&mut self.context)
            .function(NativeFunction::from_fn_ptr(console_log), js_string!("log"), 1)
            .function(NativeFunction::from_fn_ptr(console_log), js_string!("info"), 1)
            .function(NativeFunction::from_fn_ptr(console_log), js_string!("debug"), 1)
            .function(NativeFunction::from_fn_ptr(console_log), js_string!("warn"), 1)
            .function(NativeFunction::from_fn_ptr(console_log), js_string!("error"), 1)
            .build();
        self.context
            .register_global_property(js_string!("console"), console, Attribute::all())
            .expect("register console");

        // Timers: setTimeout/setInterval/clearTimeout/clearInterval + rAF.
        for (name, f) in [
            ("setTimeout", set_timeout as fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>),
            ("setInterval", set_interval),
            ("clearTimeout", clear_timer),
            ("clearInterval", clear_timer),
            ("requestAnimationFrame", request_animation_frame),
            ("cancelAnimationFrame", clear_timer),
        ] {
            let func = NativeFunction::from_fn_ptr(f).to_js_function(&self.context.realm().clone());
            self.context
                .register_global_property(js_string!(name), func, Attribute::all())
                .expect("register timer");
        }

        // fetch(url, options?)
        let fetch_fn =
            NativeFunction::from_fn_ptr(js_fetch).to_js_function(&self.context.realm().clone());
        self.context
            .register_global_property(js_string!("fetch"), fetch_fn, Attribute::all())
            .expect("register fetch");

        // XMLHttpRequest (constructable).
        let xhr_ctor = FunctionObjectBuilder::new(
            &self.context.realm().clone(),
            NativeFunction::from_fn_ptr(xhr_construct),
        )
        .name(js_string!("XMLHttpRequest"))
        .constructor(true)
        .build();
        self.context
            .register_global_property(js_string!("XMLHttpRequest"), xhr_ctor, Attribute::all())
            .expect("register XMLHttpRequest");

        // location: a live view over LOCATION_HREF.
        let realm = self.context.realm().clone();
        let location = ObjectInitializer::new(&mut self.context).build();
        let loc_accessor = |get: fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>,
                            set: Option<fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>>|
         -> PropertyDescriptor {
            let mut b = PropertyDescriptor::builder()
                .get(NativeFunction::from_fn_ptr(get).to_js_function(&realm))
                .enumerable(true)
                .configurable(true);
            if let Some(set) = set {
                b = b.set(NativeFunction::from_fn_ptr(set).to_js_function(&realm));
            }
            b.build()
        };
        location.insert_property(
            js_string!("href"),
            loc_accessor(location_get_href, Some(location_set_href)),
        );
        location.insert_property(js_string!("protocol"), loc_accessor(location_get_protocol, None));
        location.insert_property(js_string!("host"), loc_accessor(location_get_host, None));
        location.insert_property(js_string!("hostname"), loc_accessor(location_get_host, None));
        location.insert_property(js_string!("pathname"), loc_accessor(location_get_pathname, None));
        location.insert_property(js_string!("search"), loc_accessor(location_get_search, None));
        location.insert_property(js_string!("hash"), loc_accessor(location_get_hash, None));
        self.context
            .register_global_property(js_string!("location"), location, Attribute::all())
            .expect("register location");

        // localStorage: getItem/setItem/removeItem/clear/key + length.
        let storage = ObjectInitializer::new(&mut self.context)
            .function(NativeFunction::from_fn_ptr(storage_get_item), js_string!("getItem"), 1)
            .function(NativeFunction::from_fn_ptr(storage_set_item), js_string!("setItem"), 2)
            .function(NativeFunction::from_fn_ptr(storage_remove_item), js_string!("removeItem"), 1)
            .function(NativeFunction::from_fn_ptr(storage_clear), js_string!("clear"), 0)
            .function(NativeFunction::from_fn_ptr(storage_key), js_string!("key"), 1)
            .build();
        let length_getter = PropertyDescriptor::builder()
            .get(
                NativeFunction::from_fn_ptr(storage_length)
                    .to_js_function(&self.context.realm().clone()),
            )
            .enumerable(true)
            .configurable(true)
            .build();
        storage.insert_property(js_string!("length"), length_getter);
        self.context
            .register_global_property(js_string!("localStorage"), storage, Attribute::all())
            .expect("register localStorage");

        // window.parent: a chrome-side target for postMessage. Messages are
        // captured (JSON-serialized) for the runtime to drain. In a real
        // browser window.parent === window for a top-level frame, but the
        // injected navigation script posts to the embedding chrome, so a
        // distinct capturing object is closer to that intent.
        let parent = ObjectInitializer::new(&mut self.context)
            .function(NativeFunction::from_fn_ptr(post_message), js_string!("postMessage"), 2)
            .build();
        self.context
            .register_global_property(js_string!("parent"), parent, Attribute::all())
            .expect("register parent");
        // window.postMessage also captures (some scripts post to self).
        let post_fn = NativeFunction::from_fn_ptr(post_message)
            .to_js_function(&self.context.realm().clone());
        self.context
            .register_global_property(js_string!("postMessage"), post_fn, Attribute::all())
            .expect("register postMessage");

        // window aliases the global object (window.document, window.setTimeout,
        // window.location, window.parent, etc. all resolve to the globals
        // registered above).
        let global = self.context.global_object();
        self.context
            .register_global_property(js_string!("window"), global, Attribute::all())
            .expect("register window");
    }

    /// Drain the messages posted via `window.parent.postMessage` (JSON
    /// strings). The runtime parses these to handle e.g. link navigation.
    pub fn take_posted_messages(&self) -> Vec<String> {
        self.activate();
        POSTED_MESSAGES.with(|m| std::mem::take(&mut *m.borrow_mut()))
    }

    /// Set the URL exposed to script as `location`.
    pub fn set_location(&mut self, href: &str) {
        self.activate();
        LOCATION_HREF.with(|h| *h.borrow_mut() = href.to_string());
    }

    /// Snapshot the current `localStorage` contents (for per-origin
    /// persistence by the runtime).
    pub fn local_storage_entries(&self) -> Vec<(String, String)> {
        self.activate();
        LOCAL_STORAGE.with(|s| s.borrow().clone())
    }

    /// Replace `localStorage` with the given entries (seed from persistence).
    pub fn set_local_storage_entries(&mut self, entries: Vec<(String, String)>) {
        self.activate();
        LOCAL_STORAGE.with(|s| *s.borrow_mut() = entries);
    }

    /// Evaluate a script and return its completion value rendered as a
    /// display string (for smoke tests / diagnostics). Errors are returned
    /// as `Err(message)` rather than panicking.
    pub fn eval_to_string(&mut self, source: &str) -> Result<String, String> {
        self.activate();
        match self.context.eval(Source::from_bytes(source)) {
            Ok(v) => Ok(v
                .to_string(&mut self.context)
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_else(|_| "<unrenderable>".to_string())),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Drain and take the buffered `console.*` output.
    pub fn take_console_log(&self) -> Vec<String> {
        self.activate();
        CONSOLE_LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
    }

    /// The current DOM mutation generation — increments on every DOM change.
    /// The runtime compares it across a pump to skip re-layout when unchanged.
    pub fn dom_generation(&self) -> u64 {
        self.activate();
        DOM_GENERATION.with(|g| g.get())
    }

    /// Run the event loop until it settles: Boa's promise/microtask jobs plus
    /// all due timers. Timers fire in due order on a virtual clock (delays
    /// order them but do not block), so `setTimeout(f, 0)` chains resolve.
    /// `max_timer_fires` bounds runaway `setInterval` loops. Intervals are
    /// rescheduled, so this keeps the animation/interval loop running (use it
    /// when simulating ongoing time).
    pub fn run_pending(&mut self, max_timer_fires: usize) {
        self.drain(max_timer_fires, true);
    }

    /// Run the event loop as at initial page load: microtasks plus due
    /// one-shot timers (setTimeout(0) chains) and one round of pending
    /// requestAnimationFrame callbacks, but **each interval fires at most
    /// once** — a fresh document has not accrued wall-clock time, so
    /// `setInterval` must not spin at first paint (see HANDOFF: load-time
    /// event-loop semantics).
    pub fn run_initial_load(&mut self, max_timer_fires: usize) {
        self.drain(max_timer_fires, false);
    }

    /// Advance the virtual clock by one animation frame (`frame_ms`) and fire
    /// every timer/`requestAnimationFrame` callback whose due time falls within
    /// this frame, rescheduling intervals. Used by the GUI to drive continuous
    /// JS animations (rAF loops, setInterval): call once per real frame.
    /// `max_fires` bounds pathological bursts within a single frame.
    pub fn run_frame(&mut self, frame_ms: u64, max_fires: usize) {
        self.activate();
        self.context.run_jobs();
        self.pump_fetches();
        let target = VIRTUAL_CLOCK.with(|c| c.get()) + frame_ms.max(1);
        let mut fired = 0;
        while fired < max_fires {
            // Pop the earliest timer that is due within this frame window.
            let next = TIMERS.with(|t| {
                let mut v = t.borrow_mut();
                let idx = v
                    .iter()
                    .enumerate()
                    .filter(|(_, tm)| tm.due <= target)
                    .min_by_key(|(_, tm)| tm.due)
                    .map(|(i, _)| i);
                idx.map(|i| v.remove(i))
            });
            let Some(timer) = next else { break };
            VIRTUAL_CLOCK.with(|c| c.set(c.get().max(timer.due)));
            let clock = VIRTUAL_CLOCK.with(|c| c.get());
            let args: Vec<JsValue> = if timer.is_raf {
                vec![JsValue::from(clock as f64)]
            } else {
                Vec::new()
            };
            let _ = timer
                .callback
                .call(&JsValue::undefined(), &args, &mut self.context);
            if let Some(iv) = timer.interval {
                let due = clock + iv.max(1);
                TIMERS.with(|t| {
                    t.borrow_mut().push(Timer {
                        id: timer.id,
                        callback: timer.callback,
                        due,
                        interval: Some(iv),
                        is_raf: false,
                    })
                });
            }
            self.context.run_jobs();
            fired += 1;
        }
        // Advance to the frame boundary even if no timer was due, so wall-clock
        // progresses at ~real time across frames.
        VIRTUAL_CLOCK.with(|c| c.set(c.get().max(target)));
    }

    /// Whether any timers / `requestAnimationFrame` callbacks are queued — i.e.
    /// the page has an ongoing animation the GUI should keep driving frames for.
    pub fn has_pending_timers(&self) -> bool {
        self.activate();
        TIMERS.with(|t| !t.borrow().is_empty())
    }

    /// Install the host's network backend for `fetch`/XHR.
    pub fn set_fetch_engine(&mut self, engine: Box<dyn FetchEngine>) {
        self.activate();
        FETCH_ENGINE.with(|e| *e.borrow_mut() = Some(engine));
    }

    /// Whether any `fetch`/XHR requests are still awaiting their response. The
    /// runtime uses this to decide whether another layout pass is warranted.
    pub fn has_pending_fetches(&self) -> bool {
        self.activate();
        PENDING_FETCHES.with(|p| !p.borrow().is_empty())
            || PENDING_XHR.with(|p| !p.borrow().is_empty())
    }

    /// Poll in-flight fetches; settle any whose response has arrived (resolving
    /// with a `Response` or rejecting on network error), then run microtasks so
    /// `.then` callbacks execute. Returns the number settled this call.
    pub fn pump_fetches(&mut self) -> usize {
        self.activate();
        // Collect ready responses first (drains the receivers without holding
        // the borrow while we call into JS).
        let mut ready: Vec<(ResolvingFunctions, FetchResponse)> = Vec::new();
        PENDING_FETCHES.with(|p| {
            let mut v = p.borrow_mut();
            let mut i = 0;
            while i < v.len() {
                match v[i].rx.try_recv() {
                    Ok(resp) => {
                        let pf = v.remove(i);
                        ready.push((pf.resolvers, resp));
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => i += 1,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        // Worker dropped without sending — reject.
                        let pf = v.remove(i);
                        ready.push((
                            pf.resolvers,
                            FetchResponse {
                                ok: false,
                                status: 0,
                                status_text: String::new(),
                                url: String::new(),
                                body: String::new(),
                                error: Some("fetch worker disconnected".to_string()),
                            },
                        ));
                    }
                }
            }
        });
        let settled = ready.len();
        for (resolvers, resp) in ready {
            if let Some(err) = resp.error {
                let e = JsValue::from(js_string!(err.as_str()));
                let _ = resolvers.reject.call(&JsValue::undefined(), &[e], &mut self.context);
            } else {
                let response = make_response(resp, &mut self.context);
                let _ = resolvers
                    .resolve
                    .call(&JsValue::undefined(), &[JsValue::from(response)], &mut self.context);
            }
        }
        let xhr_settled = self.pump_xhr();
        if settled > 0 || xhr_settled > 0 {
            self.context.run_jobs();
        }
        settled + xhr_settled
    }

    /// Poll in-flight XHRs; for each completed one, update its object
    /// (status/statusText/responseText/response/readyState=4) and fire its
    /// `onreadystatechange` then `onload` (or `onerror` on network failure).
    fn pump_xhr(&mut self) -> usize {
        let mut ready: Vec<(JsObject, FetchResponse)> = Vec::new();
        PENDING_XHR.with(|p| {
            let mut v = p.borrow_mut();
            let mut i = 0;
            while i < v.len() {
                match v[i].rx.try_recv() {
                    Ok(resp) => {
                        let px = v.remove(i);
                        ready.push((px.obj, resp));
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => i += 1,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        let px = v.remove(i);
                        ready.push((
                            px.obj,
                            FetchResponse {
                                ok: false,
                                status: 0,
                                status_text: String::new(),
                                url: String::new(),
                                body: String::new(),
                                error: Some("xhr worker disconnected".to_string()),
                            },
                        ));
                    }
                }
            }
        });
        let settled = ready.len();
        for (obj, resp) in ready {
            let is_error = resp.error.is_some();
            let set = |obj: &JsObject, key, val: JsValue, ctx: &mut Context| {
                let _ = obj.set(key, val, false, ctx);
            };
            set(&obj, js_string!("status"), JsValue::from(resp.status), &mut self.context);
            set(
                &obj,
                js_string!("statusText"),
                JsValue::from(js_string!(resp.status_text.as_str())),
                &mut self.context,
            );
            set(
                &obj,
                js_string!("responseText"),
                JsValue::from(js_string!(resp.body.as_str())),
                &mut self.context,
            );
            set(
                &obj,
                js_string!("response"),
                JsValue::from(js_string!(resp.body.as_str())),
                &mut self.context,
            );
            set(&obj, js_string!("readyState"), JsValue::from(4u32), &mut self.context);

            let this = JsValue::from(obj.clone());
            // onreadystatechange fires on every state change; here the terminal one.
            for handler in ["onreadystatechange", if is_error { "onerror" } else { "onload" }] {
                if let Ok(cb) = obj.get(js_string!(handler), &mut self.context) {
                    if let Some(func) = cb.as_object().filter(|o| o.is_callable()) {
                        let _ = func.call(&this, &[], &mut self.context);
                    }
                }
            }
        }
        settled
    }

    fn drain(&mut self, max_timer_fires: usize, reschedule_intervals: bool) {
        self.activate();
        self.context.run_jobs();
        self.pump_fetches();
        let mut fired = 0;
        while fired < max_timer_fires {
            // Pop the earliest-due timer.
            let next = TIMERS.with(|t| {
                let mut v = t.borrow_mut();
                if v.is_empty() {
                    return None;
                }
                let (idx, _) = v
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, tm)| tm.due)
                    .map(|(i, tm)| (i, tm.due))
                    .unwrap();
                Some(v.remove(idx))
            });
            let Some(timer) = next else { break };
            VIRTUAL_CLOCK.with(|c| c.set(c.get().max(timer.due)));
            let clock = VIRTUAL_CLOCK.with(|c| c.get());
            // requestAnimationFrame callbacks receive the current timestamp.
            let args: Vec<JsValue> = if timer.is_raf {
                vec![JsValue::from(clock as f64)]
            } else {
                Vec::new()
            };
            let _ = timer
                .callback
                .call(&JsValue::undefined(), &args, &mut self.context);
            // Reschedule intervals relative to the virtual clock — unless this
            // is an initial-load drain, where each interval fires at most once.
            if reschedule_intervals {
                if let Some(iv) = timer.interval {
                    let due = clock + iv.max(1);
                    TIMERS.with(|t| {
                        t.borrow_mut().push(Timer {
                            id: timer.id,
                            callback: timer.callback,
                            due,
                            interval: Some(iv),
                            is_raf: false,
                        })
                    });
                }
            }
            self.context.run_jobs();
            fired += 1;
        }
    }
}

fn storage_get_item(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let key = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    Ok(match ls_get(&key) {
        Some(v) => JsValue::from(js_string!(v.as_str())),
        None => JsValue::null(),
    })
}
fn storage_set_item(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let key = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    let value = a.get(1).cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    ls_set(&key, &value);
    Ok(JsValue::undefined())
}
fn storage_remove_item(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let key = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    ls_remove(&key);
    Ok(JsValue::undefined())
}
fn storage_clear(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    LOCAL_STORAGE.with(|s| s.borrow_mut().clear());
    Ok(JsValue::undefined())
}
fn storage_key(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let idx = a.first().map(|v| v.to_number(c)).transpose()?.unwrap_or(0.0) as usize;
    Ok(LOCAL_STORAGE.with(|s| match s.borrow().get(idx) {
        Some((k, _)) => JsValue::from(js_string!(k.as_str())),
        None => JsValue::null(),
    }))
}
fn storage_length(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(LOCAL_STORAGE.with(|s| s.borrow().len()) as u32))
}

fn location_href() -> String {
    LOCATION_HREF.with(|h| h.borrow().clone())
}
fn location_get_href(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(js_string!(location_href().as_str())))
}
fn location_set_href(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let href = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    LOCATION_HREF.with(|h| *h.borrow_mut() = href);
    Ok(JsValue::undefined())
}
fn location_get_protocol(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(js_string!(parse_url_parts(&location_href()).0.as_str())))
}
fn location_get_host(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(js_string!(parse_url_parts(&location_href()).1.as_str())))
}
fn location_get_pathname(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(js_string!(parse_url_parts(&location_href()).2.as_str())))
}
fn location_get_search(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(js_string!(parse_url_parts(&location_href()).3.as_str())))
}
fn location_get_hash(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(js_string!(parse_url_parts(&location_href()).4.as_str())))
}

fn console_log(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let parts: Vec<String> = args
        .iter()
        .map(|v| {
            v.to_string(ctx)
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_else(|_| "<unrenderable>".to_string())
        })
        .collect();
    CONSOLE_LOG.with(|l| l.borrow_mut().push(parts.join(" ")));
    Ok(JsValue::undefined())
}

fn schedule_timer(args: &[JsValue], ctx: &mut Context, repeat: bool) -> JsResult<JsValue> {
    let Some(cb) = args.first().and_then(|v| v.as_object()).cloned() else {
        return Ok(JsValue::from(0));
    };
    let delay = args
        .get(1)
        .map(|v| v.to_number(ctx))
        .transpose()?
        .unwrap_or(0.0)
        .max(0.0) as u64;
    let id = NEXT_TIMER_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    let due = VIRTUAL_CLOCK.with(|c| c.get()) + delay;
    TIMERS.with(|t| {
        t.borrow_mut().push(Timer {
            id,
            callback: cb,
            due,
            interval: if repeat { Some(delay) } else { None },
            is_raf: false,
        })
    });
    Ok(JsValue::from(id))
}

fn set_timeout(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    schedule_timer(args, ctx, false)
}
fn set_interval(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    schedule_timer(args, ctx, true)
}
fn clear_timer(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let id = args.first().map(|v| v.to_number(ctx)).transpose()?.unwrap_or(0.0) as u32;
    TIMERS.with(|t| t.borrow_mut().retain(|tm| tm.id != id));
    Ok(JsValue::undefined())
}

/// `requestAnimationFrame(cb)` — schedule `cb` to run on the next frame
/// (~16ms of virtual time). The callback receives the frame timestamp. Shares
/// the timer queue so `run_pending` drives it; `cancelAnimationFrame(id)`
/// cancels via the same id space.
fn request_animation_frame(_this: &JsValue, args: &[JsValue], _ctx: &mut Context) -> JsResult<JsValue> {
    let Some(cb) = args.first().and_then(|v| v.as_object()).cloned() else {
        return Ok(JsValue::from(0));
    };
    let id = NEXT_TIMER_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    let due = VIRTUAL_CLOCK.with(|c| c.get()) + 16;
    TIMERS.with(|t| {
        t.borrow_mut().push(Timer {
            id,
            callback: cb,
            due,
            interval: None,
            is_raf: true,
        })
    });
    Ok(JsValue::from(id))
}

/// `fetch(url, options?)` → Promise<Response>. Delegates the network IO to the
/// host's FetchEngine (worker thread); the returned promise settles when
/// `pump_fetches` sees the response. With no engine installed, rejects.
/// Read a plain `{name: value}` headers object into ordered (name, value)
/// pairs. Non-object values (undefined/null) yield an empty list.
fn read_header_object(value: &JsValue, ctx: &mut Context) -> Vec<(String, String)> {
    let Some(obj) = value.as_object() else {
        return Vec::new();
    };
    let Ok(keys) = obj.own_property_keys(ctx) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for key in keys {
        let name = key.to_string();
        if let Ok(v) = obj.get(key, ctx) {
            if let Ok(s) = v.to_string(ctx) {
                out.push((name, s.to_std_string_escaped()));
            }
        }
    }
    out
}

fn js_fetch(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let url = args.first().cloned().unwrap_or_default().to_string(ctx)?.to_std_string_escaped();
    // options.method / options.body / options.headers
    let (method, body, headers) = match args.get(1).filter(|v| v.is_object()) {
        Some(opts) => {
            let o = opts.as_object().unwrap();
            let method = o
                .get(js_string!("method"), ctx)?
                .to_string(ctx)
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_else(|_| "GET".to_string());
            let body = {
                let b = o.get(js_string!("body"), ctx)?;
                if b.is_undefined() || b.is_null() {
                    None
                } else {
                    Some(b.to_string(ctx)?.to_std_string_escaped())
                }
            };
            let headers = read_header_object(&o.get(js_string!("headers"), ctx)?, ctx);
            (method, body, headers)
        }
        None => ("GET".to_string(), None, Vec::new()),
    };

    let (promise, resolvers) = JsPromise::new_pending(ctx);
    let started = FETCH_ENGINE.with(|e| {
        e.borrow().as_ref().map(|eng| {
            eng.start(FetchRequest {
                url: url.clone(),
                method: method.clone(),
                body: body.clone(),
                headers: headers.clone(),
            })
        })
    });
    match started {
        Some(rx) => {
            PENDING_FETCHES.with(|p| p.borrow_mut().push(PendingFetch { rx, resolvers }));
        }
        None => {
            // No network backend — reject immediately.
            let e = JsValue::from(js_string!("fetch: no network backend"));
            resolvers.reject.call(&JsValue::undefined(), &[e], ctx)?;
        }
    }
    Ok(JsValue::from(promise))
}

/// Build a `Response` object from a completed fetch: ok/status/statusText/url
/// data properties plus `text()`/`json()` methods returning resolved promises.
fn make_response(resp: FetchResponse, ctx: &mut Context) -> JsObject {
    let realm = ctx.realm().clone();
    let obj = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(response_text), js_string!("text"), 0)
        .function(NativeFunction::from_fn_ptr(response_json), js_string!("json"), 0)
        .build();
    let data = |v: JsValue| {
        PropertyDescriptor::builder()
            .value(v)
            .writable(false)
            .enumerable(true)
            .configurable(true)
            .build()
    };
    obj.insert_property(js_string!("ok"), data(JsValue::from(resp.ok)));
    obj.insert_property(js_string!("status"), data(JsValue::from(resp.status)));
    obj.insert_property(
        js_string!("statusText"),
        data(JsValue::from(js_string!(resp.status_text.as_str()))),
    );
    obj.insert_property(
        js_string!("url"),
        data(JsValue::from(js_string!(resp.url.as_str()))),
    );
    // Body is stashed on a non-enumerable slot for text()/json() to read.
    obj.insert_property(
        js_string!("_body"),
        PropertyDescriptor::builder()
            .value(JsValue::from(js_string!(resp.body.as_str())))
            .writable(false)
            .enumerable(false)
            .configurable(false)
            .build(),
    );
    let _ = realm;
    obj
}

fn response_body(this: &JsValue, ctx: &mut Context) -> String {
    this.as_object()
        .and_then(|o| o.get(js_string!("_body"), ctx).ok())
        .and_then(|v| v.to_string(ctx).ok())
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_default()
}

fn response_text(this: &JsValue, _a: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let body = response_body(this, ctx);
    Ok(JsValue::from(JsPromise::resolve(
        JsValue::from(js_string!(body.as_str())),
        ctx,
    )))
}

fn response_json(this: &JsValue, _a: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let body = response_body(this, ctx);
    // Parse via the built-in JSON.parse; a parse error rejects the promise.
    let json = ctx.global_object().get(js_string!("JSON"), ctx)?;
    let parse = json
        .as_object()
        .and_then(|o| o.get(js_string!("parse"), ctx).ok())
        .and_then(|v| v.as_object().cloned());
    let parsed = match parse {
        Some(func) => func.call(&json, &[JsValue::from(js_string!(body.as_str()))], ctx),
        None => Ok(JsValue::null()),
    };
    Ok(match parsed {
        Ok(v) => JsValue::from(JsPromise::resolve(v, ctx)),
        Err(e) => JsValue::from(JsPromise::reject(e, ctx)),
    })
}

/// `new XMLHttpRequest()` — build an XHR instance with XhrState host data,
/// the open/send/setRequestHeader methods, and the initial readonly-ish state
/// (readyState 0, status 0, empty responseText, null handlers).
fn xhr_construct(_this: &JsValue, _a: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let obj = JsObject::from_proto_and_data(None, XhrState::default());
    let realm = ctx.realm().clone();
    let method = |f: fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>, name, len| {
        let desc = PropertyDescriptor::builder()
            .value(NativeFunction::from_fn_ptr(f).to_js_function(&realm))
            .writable(true)
            .enumerable(false)
            .configurable(true)
            .build();
        obj.insert_property(name, desc);
        let _ = len;
    };
    method(xhr_open, js_string!("open"), 2);
    method(xhr_send, js_string!("send"), 1);
    method(xhr_set_request_header, js_string!("setRequestHeader"), 2);
    method(xhr_abort, js_string!("abort"), 0);

    let data = |v: JsValue| {
        PropertyDescriptor::builder()
            .value(v)
            .writable(true)
            .enumerable(true)
            .configurable(true)
            .build()
    };
    obj.insert_property(js_string!("readyState"), data(JsValue::from(0u32)));
    obj.insert_property(js_string!("status"), data(JsValue::from(0u32)));
    obj.insert_property(js_string!("statusText"), data(JsValue::from(js_string!(""))));
    obj.insert_property(js_string!("responseText"), data(JsValue::from(js_string!(""))));
    obj.insert_property(js_string!("response"), data(JsValue::from(js_string!(""))));
    obj.insert_property(js_string!("onreadystatechange"), data(JsValue::null()));
    obj.insert_property(js_string!("onload"), data(JsValue::null()));
    obj.insert_property(js_string!("onerror"), data(JsValue::null()));
    Ok(JsValue::from(obj))
}

fn xhr_state(this: &JsValue) -> Option<JsObject> {
    this.as_object()
        .filter(|o| o.downcast_ref::<XhrState>().is_some())
        .cloned()
}

fn xhr_open(this: &JsValue, a: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let Some(obj) = xhr_state(this) else {
        return Ok(JsValue::undefined());
    };
    let method = a.first().cloned().unwrap_or_default().to_string(ctx)?.to_std_string_escaped();
    let url = a.get(1).cloned().unwrap_or_default().to_string(ctx)?.to_std_string_escaped();
    if let Some(state) = obj.downcast_ref::<XhrState>() {
        *state.method.borrow_mut() = method;
        *state.url.borrow_mut() = url;
    }
    let _ = obj.set(js_string!("readyState"), JsValue::from(1u32), false, ctx);
    Ok(JsValue::undefined())
}

fn xhr_send(this: &JsValue, a: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let Some(obj) = xhr_state(this) else {
        return Ok(JsValue::undefined());
    };
    let (method, url, headers) = {
        let state = obj.downcast_ref::<XhrState>().unwrap();
        let m = state.method.borrow().clone();
        let u = state.url.borrow().clone();
        let h = state.headers.borrow().clone();
        (m, u, h)
    };
    let body = match a.first() {
        Some(v) if !v.is_undefined() && !v.is_null() => Some(v.to_string(ctx)?.to_std_string_escaped()),
        _ => None,
    };
    let started = FETCH_ENGINE.with(|e| {
        e.borrow()
            .as_ref()
            .map(|eng| eng.start(FetchRequest { url, method, body, headers }))
    });
    match started {
        Some(rx) => PENDING_XHR.with(|p| p.borrow_mut().push(PendingXhr { rx, obj })),
        None => {
            // No backend — fire onerror on the next pump-equivalent (do it now).
            let this_v = JsValue::from(obj.clone());
            if let Ok(cb) = obj.get(js_string!("onerror"), ctx) {
                if let Some(func) = cb.as_object().filter(|o| o.is_callable()) {
                    let _ = func.call(&this_v, &[], ctx);
                }
            }
        }
    }
    Ok(JsValue::undefined())
}

fn xhr_set_request_header(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let Some(obj) = xhr_state(this) else {
        return Ok(JsValue::undefined());
    };
    let name = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    let value = a.get(1).cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    if let Some(state) = obj.downcast_ref::<XhrState>() {
        state.headers.borrow_mut().push((name, value));
    }
    Ok(JsValue::undefined())
}

fn xhr_abort(this: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    // Drop any pending request for this object.
    if let Some(obj) = xhr_state(this) {
        PENDING_XHR.with(|p| p.borrow_mut().retain(|px| px.obj != obj));
    }
    Ok(JsValue::undefined())
}

fn script_dom_root() -> Option<Rc<RefCell<Node>>> {
    SCRIPT_DOM.with(|d| d.borrow().clone())
}

/// Register a document-level event listener (on the DOM root node, so bubbling
/// events from any element reach it — used by the injected navigation script's
/// `document.addEventListener('click', ...)`).
fn doc_add_event_listener(_this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let Some(root) = script_dom_root() else {
        return Ok(JsValue::undefined());
    };
    let event_type = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    let Some(cb) = a.get(1).and_then(|v| v.as_object()).cloned() else {
        return Ok(JsValue::undefined());
    };
    let capture = parse_capture(a.get(2), c);
    LISTENERS.with(|m| {
        m.borrow_mut()
            .entry(node_key(&root))
            .or_default()
            .push(Listener { event_type, callback: cb, capture });
    });
    Ok(JsValue::undefined())
}

fn doc_remove_event_listener(_this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let Some(root) = script_dom_root() else {
        return Ok(JsValue::undefined());
    };
    let event_type = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    let cb = a.get(1).and_then(|v| v.as_object()).cloned();
    LISTENERS.with(|m| {
        if let Some(v) = m.borrow_mut().get_mut(&node_key(&root)) {
            v.retain(|l| {
                l.event_type != event_type || cb.as_ref().map(|c| c != &l.callback).unwrap_or(true)
            });
        }
    });
    Ok(JsValue::undefined())
}

fn doc_dispatch_event(_this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let Some(root) = script_dom_root() else {
        return Ok(JsValue::from(true));
    };
    let event_type = match a.first() {
        Some(v) if v.is_object() => v
            .as_object()
            .unwrap()
            .get(js_string!("type"), c)?
            .to_string(c)?
            .to_std_string_escaped(),
        Some(v) => v.clone().to_string(c)?.to_std_string_escaped(),
        None => String::new(),
    };
    Ok(JsValue::from(run_dispatch(root, &event_type, None, c)))
}

/// `window.parent.postMessage(message, targetOrigin)` — capture the message
/// (JSON-serialized) for the runtime to drain.
fn post_message(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let msg = args.first().cloned().unwrap_or(JsValue::undefined());
    let serialized = json_stringify(&msg, ctx).unwrap_or_else(|| "null".to_string());
    POSTED_MESSAGES.with(|m| m.borrow_mut().push(serialized));
    Ok(JsValue::undefined())
}

/// Serialize a value via the built-in `JSON.stringify`.
fn json_stringify(value: &JsValue, ctx: &mut Context) -> Option<String> {
    let json = ctx.global_object().get(js_string!("JSON"), ctx).ok()?;
    let json_obj = json.as_object()?.clone();
    let stringify = json_obj.get(js_string!("stringify"), ctx).ok()?;
    let func = stringify.as_object()?.clone();
    let result = func.call(&json, &[value.clone()], ctx).ok()?;
    Some(result.to_string(ctx).ok()?.to_std_string_escaped())
}

fn dom_select_all_to_array(selector: &str, ctx: &mut Context) -> JsResult<JsValue> {
    let nodes = SCRIPT_DOM.with(|d| {
        d.borrow()
            .as_ref()
            .map(|dom| query_selector_all(dom.clone(), selector))
            .unwrap_or_default()
    });
    let arr = JsArray::new(ctx);
    for n in nodes {
        let el = make_element(n, ctx);
        arr.push(JsValue::from(el), ctx)?;
    }
    Ok(JsValue::from(arr))
}

fn dom_get_elements_by_tag_name(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let tag = args.first().cloned().unwrap_or_default().to_string(ctx)?.to_std_string_escaped();
    if tag == "*" {
        return dom_select_all_to_array("*", ctx);
    }
    dom_select_all_to_array(&tag, ctx)
}

fn dom_get_elements_by_class_name(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let classes = args.first().cloned().unwrap_or_default().to_string(ctx)?.to_std_string_escaped();
    // Multiple space-separated class names form a compound selector.
    let selector: String = classes.split_whitespace().map(|c| format!(".{c}")).collect();
    if selector.is_empty() {
        return Ok(JsValue::from(JsArray::new(ctx)));
    }
    dom_select_all_to_array(&selector, ctx)
}

fn dom_create_element(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let tag = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(ctx)?
        .to_std_string_escaped();
    let node = Rc::new(RefCell::new(Node::new(NodeKind::Element(Element::new(
        &tag,
        Vec::new(),
    )))));
    Ok(JsValue::from(make_element(node, ctx)))
}

fn dom_create_text_node(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let text = args
        .first()
        .cloned()
        .unwrap_or_default()
        .to_string(ctx)?
        .to_std_string_escaped();
    let node = Rc::new(RefCell::new(Node::new(NodeKind::Text(text))));
    Ok(JsValue::from(make_element(node, ctx)))
}

fn dom_get_element_by_id(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let id = args
        .first()
        .and_then(|v| v.as_string())
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_default();
    let node = SCRIPT_DOM
        .with(|d| d.borrow().as_ref().and_then(|dom| get_element_by_id(Some(dom.clone()), &id)));
    Ok(match node {
        Some(n) => JsValue::from(make_element(n, ctx)),
        None => JsValue::null(),
    })
}

fn dom_query_selector(_this: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let sel = args
        .first()
        .and_then(|v| v.as_string())
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_default();
    let node =
        SCRIPT_DOM.with(|d| d.borrow().as_ref().and_then(|dom| query_selector(dom.clone(), &sel)));
    Ok(match node {
        Some(n) => JsValue::from(make_element(n, ctx)),
        None => JsValue::null(),
    })
}

fn dom_query_selector_all(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let sel = args
        .first()
        .and_then(|v| v.as_string())
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_default();
    let nodes = SCRIPT_DOM.with(|d| {
        d.borrow()
            .as_ref()
            .map(|dom| query_selector_all(dom.clone(), &sel))
            .unwrap_or_default()
    });
    let array = JsArray::new(ctx);
    for n in nodes {
        array.push(make_element(n, ctx), ctx)?;
    }
    Ok(array.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmo_engine::renderer::html::parser::HtmlParser;
    use cosmo_engine::renderer::html::token::HtmlTokenizer;

    #[test]
    fn evaluates_arithmetic_and_strings() {
        let mut host = ScriptHost::new();
        assert_eq!(host.eval_to_string("1 + 2 * 3").unwrap(), "7");
        assert_eq!(
            host.eval_to_string("['a','b','c'].join('-')").unwrap(),
            "a-b-c"
        );
    }

    #[test]
    fn control_flow_and_functions() {
        let mut host = ScriptHost::new();
        let src = "function fib(n){ return n<2 ? n : fib(n-1)+fib(n-2); } fib(10)";
        assert_eq!(host.eval_to_string(src).unwrap(), "55");
    }

    #[test]
    fn two_hosts_keep_independent_state() {
        // Plan D5: per-page state lives on each host, so two LivePages on the
        // same thread don't clobber each other (the active page is swapped in
        // on each call).
        let html_a = "<html><body><div id=\"x\">A-doc</div></body></html>";
        let html_b = "<html><body><div id=\"x\">B-doc</div></body></html>";
        let doc_a = HtmlParser::new(HtmlTokenizer::new(html_a.to_string())).construct_tree().borrow().document();
        let doc_b = HtmlParser::new(HtmlTokenizer::new(html_b.to_string())).construct_tree().borrow().document();

        let mut a = ScriptHost::new();
        let mut b = ScriptHost::new();
        a.set_location("https://a.test/");
        b.set_location("https://b.test/");
        a.set_document(doc_a);
        b.set_document(doc_b);

        // Interleave: each host sees its own document, location, and variables.
        a.eval_to_string("globalThis.who = 'A';").unwrap();
        b.eval_to_string("globalThis.who = 'B';").unwrap();
        assert_eq!(a.eval_to_string("document.getElementById('x').textContent").unwrap(), "A-doc");
        assert_eq!(b.eval_to_string("document.getElementById('x').textContent").unwrap(), "B-doc");
        assert_eq!(a.eval_to_string("globalThis.who").unwrap(), "A");
        assert_eq!(b.eval_to_string("globalThis.who").unwrap(), "B");
        assert_eq!(a.eval_to_string("location.host").unwrap(), "a.test");
        assert_eq!(b.eval_to_string("location.host").unwrap(), "b.test");

        // Mutations in A do not affect B's DOM.
        a.eval_to_string("document.getElementById('x').textContent = 'A-changed';").unwrap();
        assert_eq!(a.eval_to_string("document.getElementById('x').textContent").unwrap(), "A-changed");
        assert_eq!(b.eval_to_string("document.getElementById('x').textContent").unwrap(), "B-doc");
    }

    #[test]
    fn dom_generation_tracks_mutations() {
        let html = "<html><body><div id=\"b\">x</div><ul id=\"l\"></ul></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document);

        let g0 = host.dom_generation();
        // A pure read does not bump the generation.
        host.eval_to_string("document.getElementById('b').textContent").unwrap();
        assert_eq!(host.dom_generation(), g0, "reads must not bump the generation");

        // Each mutation bumps it.
        host.eval_to_string("document.getElementById('b').setAttribute('data-x','1');").unwrap();
        let g1 = host.dom_generation();
        assert!(g1 > g0, "setAttribute should bump");

        host.eval_to_string("document.getElementById('b').textContent = 'y';").unwrap();
        let g2 = host.dom_generation();
        assert!(g2 > g1, "textContent set should bump");

        host.eval_to_string(
            "document.getElementById('l').appendChild(document.createElement('li'));",
        )
        .unwrap();
        assert!(host.dom_generation() > g2, "appendChild should bump");
    }

    #[test]
    fn watchdog_bounds_runaway_loop() {
        let mut host = ScriptHost::new();
        // Lower the cap so the test fails fast (the production default is far
        // higher; here we just verify the watchdog fires and stays isolated).
        host.set_loop_iteration_limit(50_000);
        let result = host.eval_to_string("var i=0; while(true){ i++; }");
        assert!(result.is_err(), "runaway loop should hit the iteration limit");
        // The host stays usable afterward (the error is isolated).
        assert_eq!(host.eval_to_string("1 + 2").unwrap(), "3");
        // Legitimate bounded loops well under the cap still work.
        assert_eq!(
            host.eval_to_string("var s=0; for(var k=0;k<1000;k++){s+=k;} s").unwrap(),
            "499500"
        );
    }

    #[test]
    fn state_persists_across_evals() {
        let mut host = ScriptHost::new();
        host.eval_to_string("var counter = 0;").unwrap();
        host.eval_to_string("counter += 5;").unwrap();
        assert_eq!(host.eval_to_string("counter").unwrap(), "5");
    }

    #[test]
    fn get_element_by_id_reads_textcontent() {
        let html =
            "<html><body><div id=\"greeting\">Hello DOM</div><p id=\"x\">other</p></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();

        let mut host = ScriptHost::new();
        host.set_document(document);
        assert_eq!(
            host.eval_to_string("document.getElementById('greeting').textContent")
                .unwrap(),
            "Hello DOM"
        );
        assert_eq!(
            host.eval_to_string("document.getElementById('missing')")
                .unwrap(),
            "null"
        );
        assert_eq!(
            host.eval_to_string("document.getElementById('greeting').textContent.length")
                .unwrap(),
            "9"
        );
    }

    #[test]
    fn query_selector_reuses_engine_matcher() {
        let html = r##"<html><body>
            <div class="card"><p class="title">First</p><a href="#">link</a></div>
            <div class="card"><p class="title">Second</p></div>
            <span id="lone">x</span>
        </body></html>"##;
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document);

        // First match, in document order.
        assert_eq!(
            host.eval_to_string("document.querySelector('.title').textContent").unwrap(),
            "First"
        );
        // Descendant combinator + id.
        assert_eq!(
            host.eval_to_string("document.querySelector('.card .title').textContent").unwrap(),
            "First"
        );
        assert_eq!(
            host.eval_to_string("document.querySelector('#lone').tagName").unwrap(),
            "SPAN"
        );
        // querySelectorAll returns an array; length + element access work.
        assert_eq!(
            host.eval_to_string("document.querySelectorAll('.title').length").unwrap(),
            "2"
        );
        assert_eq!(
            host.eval_to_string("document.querySelectorAll('.title')[1].textContent").unwrap(),
            "Second"
        );
        // Selector list (union).
        assert_eq!(
            host.eval_to_string("document.querySelectorAll('a, span').length").unwrap(),
            "2"
        );
        // No match: querySelector null, querySelectorAll empty.
        assert_eq!(host.eval_to_string("document.querySelector('.nope')").unwrap(), "null");
        assert_eq!(
            host.eval_to_string("document.querySelectorAll('.nope').length").unwrap(),
            "0"
        );
    }

    #[test]
    fn element_attributes_read_and_write() {
        let html = "<html><body><div id=\"box\" class=\"a b\" data-role=\"panel\">x</div></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document);

        assert_eq!(host.eval_to_string("document.getElementById('box').id").unwrap(), "box");
        assert_eq!(host.eval_to_string("document.getElementById('box').className").unwrap(), "a b");
        assert_eq!(host.eval_to_string("document.getElementById('box').tagName").unwrap(), "DIV");
        assert_eq!(
            host.eval_to_string("document.getElementById('box').getAttribute('data-role')").unwrap(),
            "panel"
        );
        assert_eq!(
            host.eval_to_string("document.getElementById('box').hasAttribute('data-role')").unwrap(),
            "true"
        );
        assert_eq!(
            host.eval_to_string("document.getElementById('box').getAttribute('missing')").unwrap(),
            "null"
        );
        // Write className and a custom attribute, read back.
        host.eval_to_string("document.getElementById('box').className = 'c';").unwrap();
        assert_eq!(host.eval_to_string("document.getElementById('box').className").unwrap(), "c");
        host.eval_to_string("document.getElementById('box').setAttribute('data-n', '42');").unwrap();
        assert_eq!(
            host.eval_to_string("document.getElementById('box').getAttribute('data-n')").unwrap(),
            "42"
        );
    }

    #[test]
    fn set_textcontent_mutates_the_dom() {
        let html = "<html><body><div id=\"out\">initial</div></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();

        let mut host = ScriptHost::new();
        host.set_document(document.clone());
        // Mutate via JS, then read back through a fresh binding.
        host.eval_to_string("document.getElementById('out').textContent = 'changed by JS';")
            .unwrap();
        assert_eq!(
            host.eval_to_string("document.getElementById('out').textContent")
                .unwrap(),
            "changed by JS"
        );
        // The change is visible in the real DOM (not just the JS view).
        let el = get_element_by_id(Some(document), &"out".to_string()).unwrap();
        let mut s = String::new();
        collect_text(el.borrow().first_child(), &mut s);
        assert_eq!(s, "changed by JS");
    }

    #[test]
    fn create_and_mutate_the_tree() {
        let html = "<html><body><ul id=\"list\"><li id=\"a\">A</li></ul></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document);

        // createElement + createTextNode + appendChild builds a new <li>B</li>.
        host.eval_to_string(
            "var li = document.createElement('li'); \
             li.appendChild(document.createTextNode('B')); \
             document.getElementById('list').appendChild(li);",
        )
        .unwrap();
        assert_eq!(host.eval_to_string("document.getElementById('list').textContent").unwrap(), "AB");

        // insertBefore places a new node ahead of the first child.
        host.eval_to_string(
            "var first = document.getElementById('a'); \
             var c = document.createElement('li'); c.textContent = 'C'; \
             document.getElementById('list').insertBefore(c, first);",
        )
        .unwrap();
        assert_eq!(host.eval_to_string("document.getElementById('list').textContent").unwrap(), "CAB");

        // removeChild drops the original first <li>.
        host.eval_to_string(
            "document.getElementById('list').removeChild(document.getElementById('a'));",
        )
        .unwrap();
        assert_eq!(host.eval_to_string("document.getElementById('list').textContent").unwrap(), "CB");

        // querySelectorAll sees the current tree.
        assert_eq!(host.eval_to_string("document.querySelectorAll('li').length").unwrap(), "2");
    }

    #[test]
    fn todo_app_end_to_end() {
        // A miniature TodoMVC: an "add" button appends <li> items; clicking an
        // item toggles a 'done' class; a per-item delete removes it. Exercises
        // createElement, appendChild, addEventListener, dispatchEvent (bubbling),
        // classList, textContent, removeChild and querySelectorAll together.
        let html = "<html><body>\
            <button id=\"add\">add</button>\
            <ul id=\"todos\"></ul>\
            </body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document.clone());

        host.eval_to_string(
            "var seq = 0; \
             function addTodo(text) { \
                 var li = document.createElement('li'); \
                 li.className = 'todo'; \
                 li.textContent = text; \
                 li.addEventListener('click', function(e) { \
                     e.target.classList.toggle('done'); \
                 }); \
                 document.getElementById('todos').appendChild(li); \
                 return li; \
             } \
             document.getElementById('add').addEventListener('click', function() { \
                 addTodo('item ' + (++seq)); \
             });",
        )
        .unwrap();

        // Click "add" three times -> three todos.
        let add = get_element_by_id(Some(document.clone()), &"add".to_string()).unwrap();
        for _ in 0..3 {
            host.dispatch_event(add.clone(), "click");
        }
        assert_eq!(host.eval_to_string("document.querySelectorAll('.todo').length").unwrap(), "3");
        assert_eq!(
            host.eval_to_string("document.getElementById('todos').textContent").unwrap(),
            "item 1item 2item 3"
        );

        // Click the second todo -> toggles 'done'.
        host.eval_to_string(
            "document.querySelectorAll('.todo')[1].dispatchEvent({type:'click'});",
        )
        .unwrap();
        assert_eq!(host.eval_to_string("document.querySelectorAll('.done').length").unwrap(), "1");
        assert_eq!(
            host.eval_to_string("document.querySelectorAll('.done')[0].textContent").unwrap(),
            "item 2"
        );

        // Remove the first todo.
        host.eval_to_string(
            "var list = document.getElementById('todos'); \
             list.removeChild(list.children[0]);",
        )
        .unwrap();
        assert_eq!(host.eval_to_string("document.querySelectorAll('.todo').length").unwrap(), "2");
        assert_eq!(
            host.eval_to_string("document.getElementById('todos').textContent").unwrap(),
            "item 2item 3"
        );
        // The toggled item survived removal with its state intact.
        assert_eq!(host.eval_to_string("document.querySelectorAll('.done').length").unwrap(), "1");
    }

    #[test]
    fn injected_navigation_script_posts_message() {
        // Mirrors loader.rs's injected link-interception script: a document
        // click listener finds the closest <a>, prevents default, and posts a
        // navigate message to window.parent.
        let html = "<html><body><div><a href=\"/next\" id=\"lnk\">go</a></div></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document.clone());

        host.eval_to_string(
            "document.addEventListener('click', function(event) { \
                 var anchor = event.target && event.target.closest ? event.target.closest('a') : null; \
                 if (!anchor) return; \
                 if (event.defaultPrevented) return; \
                 var href = anchor.getAttribute('href'); \
                 if (!href) return; \
                 event.preventDefault(); \
                 window.parent.postMessage({type:'cosmobrowse:navigate', href:href, target:anchor.getAttribute('target')||''}, '*'); \
             });",
        )
        .unwrap();

        // Dispatch a click on the anchor (as the runtime would on a real click).
        let lnk = get_element_by_id(Some(document), &"lnk".to_string()).unwrap();
        let default_ran = host.dispatch_event(lnk, "click");
        assert!(!default_ran, "preventDefault should suppress the default action");

        let msgs = host.take_posted_messages();
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].contains("cosmobrowse:navigate"), "got: {}", msgs[0]);
        assert!(msgs[0].contains("/next"), "got: {}", msgs[0]);
        // Drained.
        assert!(host.take_posted_messages().is_empty());
    }

    #[test]
    fn local_storage_and_snapshot() {
        let html = "<html><body></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_local_storage_entries(vec![]);
        host.set_document(document);

        host.eval_to_string("localStorage.setItem('a', '1'); localStorage.setItem('b', '2');").unwrap();
        assert_eq!(host.eval_to_string("localStorage.getItem('a')").unwrap(), "1");
        assert_eq!(host.eval_to_string("localStorage.length").unwrap(), "2");
        assert_eq!(host.eval_to_string("localStorage.key(1)").unwrap(), "b");
        assert_eq!(host.eval_to_string("localStorage.getItem('missing')").unwrap(), "null");

        // Overwrite keeps position; remove drops it.
        host.eval_to_string("localStorage.setItem('a', '9'); localStorage.removeItem('b');").unwrap();
        assert_eq!(host.eval_to_string("localStorage.getItem('a')").unwrap(), "9");
        assert_eq!(host.eval_to_string("localStorage.length").unwrap(), "1");

        // Snapshot reflects the current state; window.localStorage is the same.
        assert_eq!(host.local_storage_entries(), vec![("a".to_string(), "9".to_string())]);
        assert_eq!(host.eval_to_string("window.localStorage.getItem('a')").unwrap(), "9");

        // Restore replaces contents.
        host.set_local_storage_entries(vec![("x".to_string(), "y".to_string())]);
        assert_eq!(host.eval_to_string("localStorage.getItem('x')").unwrap(), "y");
        assert_eq!(host.eval_to_string("localStorage.length").unwrap(), "1");
    }

    #[test]
    fn window_and_location() {
        let html = "<html><body><div id=\"x\">hi</div></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document);
        host.set_location("https://example.com/path/page?q=1#frag");

        // window aliases the global object.
        assert_eq!(host.eval_to_string("window.document === document").unwrap(), "true");
        assert_eq!(
            host.eval_to_string("window.document.getElementById('x').textContent").unwrap(),
            "hi"
        );
        // location parts.
        assert_eq!(host.eval_to_string("location.href").unwrap(), "https://example.com/path/page?q=1#frag");
        assert_eq!(host.eval_to_string("location.protocol").unwrap(), "https:");
        assert_eq!(host.eval_to_string("location.host").unwrap(), "example.com");
        assert_eq!(host.eval_to_string("location.pathname").unwrap(), "/path/page");
        assert_eq!(host.eval_to_string("location.search").unwrap(), "?q=1");
        assert_eq!(host.eval_to_string("location.hash").unwrap(), "#frag");
        assert_eq!(host.eval_to_string("window.location.host").unwrap(), "example.com");
        // Assigning href updates the view.
        host.eval_to_string("location.href = 'https://a.test/b';").unwrap();
        assert_eq!(host.eval_to_string("location.pathname").unwrap(), "/b");
    }

    #[test]
    fn inner_html_get_set_and_collections() {
        let html = "<html><body><ul id=\"list\"><li class=\"item\">A</li></ul></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document);

        // Getter serializes children.
        assert_eq!(
            host.eval_to_string("document.getElementById('list').innerHTML").unwrap(),
            "<li class=\"item\">A</li>"
        );
        // Setter parses a fragment and replaces children.
        host.eval_to_string(
            "document.getElementById('list').innerHTML = \
             '<li class=\"item\">X</li><li class=\"item\">Y</li>';",
        )
        .unwrap();
        assert_eq!(host.eval_to_string("document.getElementById('list').textContent").unwrap(), "XY");
        assert_eq!(
            host.eval_to_string("document.getElementsByTagName('li').length").unwrap(),
            "2"
        );
        assert_eq!(
            host.eval_to_string("document.getElementsByClassName('item').length").unwrap(),
            "2"
        );
        // The parsed nodes are live: querySelector sees them.
        assert_eq!(
            host.eval_to_string("document.querySelectorAll('.item').length").unwrap(),
            "2"
        );
        // Text content is HTML-escaped when serialized (no markup injection on
        // round-trip).
        host.eval_to_string("document.getElementById('list').textContent = '1 < 2 & 3';").unwrap();
        assert_eq!(
            host.eval_to_string("document.getElementById('list').innerHTML").unwrap(),
            "1 &lt; 2 &amp; 3"
        );
    }

    #[test]
    fn matches_and_closest() {
        let html = "<html><body><div class=\"card\" id=\"c\">\
                    <p class=\"title\"><a href=\"/x\" id=\"lnk\">go</a></p></div></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document);

        assert_eq!(host.eval_to_string("document.getElementById('c').matches('.card')").unwrap(), "true");
        assert_eq!(host.eval_to_string("document.getElementById('c').matches('.nope')").unwrap(), "false");
        // closest walks up to the nearest matching ancestor.
        assert_eq!(
            host.eval_to_string("document.getElementById('lnk').closest('.card').id").unwrap(),
            "c"
        );
        assert_eq!(
            host.eval_to_string("document.getElementById('lnk').closest('a').id").unwrap(),
            "lnk"
        );
        assert_eq!(
            host.eval_to_string("document.getElementById('lnk').closest('.missing')").unwrap(),
            "null"
        );
        // Element-scoped querySelector searches within the subtree.
        assert_eq!(
            host.eval_to_string("document.getElementById('c').querySelector('a').getAttribute('href')").unwrap(),
            "/x"
        );
        assert_eq!(
            host.eval_to_string("document.getElementById('c').querySelectorAll('a').length").unwrap(),
            "1"
        );
        // Descendant-only scoping: the element itself never matches, even when
        // it satisfies the selector (DOM spec).
        assert_eq!(
            host.eval_to_string("document.getElementById('c').querySelector('.card')").unwrap(),
            "null"
        );
        assert_eq!(
            host.eval_to_string("document.getElementById('c').querySelectorAll('.card').length").unwrap(),
            "0"
        );
    }

    #[test]
    fn request_animation_frame_runs_via_event_loop() {
        let html = "<html><body><p id=\"out\"></p></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document);

        // A rAF callback receives a timestamp and can chain another frame.
        host.eval_to_string(
            "var frames = 0; \
             function tick(ts) { \
                 frames++; \
                 document.getElementById('out').textContent += (typeof ts === 'number' ? 'n' : '?'); \
                 if (frames < 3) requestAnimationFrame(tick); \
             } \
             requestAnimationFrame(tick);",
        )
        .unwrap();
        assert_eq!(host.eval_to_string("frames").unwrap(), "0");
        host.run_pending(100);
        assert_eq!(host.eval_to_string("frames").unwrap(), "3");
        assert_eq!(host.eval_to_string("document.getElementById('out').textContent").unwrap(), "nnn");

        // cancelAnimationFrame prevents a scheduled frame.
        host.eval_to_string(
            "var id = requestAnimationFrame(function(){ frames++; }); cancelAnimationFrame(id);",
        )
        .unwrap();
        host.run_pending(100);
        assert_eq!(host.eval_to_string("frames").unwrap(), "3");
    }

    #[test]
    fn dataset_reads_and_writes_data_attributes() {
        let html = "<html><body><div id=\"b\" data-user-id=\"42\" data-role=\"admin\">x</div></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document);

        // Read: data-user-id -> dataset.userId (camelCase).
        assert_eq!(host.eval_to_string("document.getElementById('b').dataset.userId").unwrap(), "42");
        assert_eq!(host.eval_to_string("document.getElementById('b').dataset.role").unwrap(), "admin");
        assert_eq!(host.eval_to_string("'userId' in document.getElementById('b').dataset").unwrap(), "true");
        assert_eq!(host.eval_to_string("'missing' in document.getElementById('b').dataset").unwrap(), "false");

        // Write: dataset.fooBar -> data-foo-bar attribute.
        host.eval_to_string("document.getElementById('b').dataset.fooBar = 'yes';").unwrap();
        assert_eq!(
            host.eval_to_string("document.getElementById('b').getAttribute('data-foo-bar')").unwrap(),
            "yes"
        );
        // Update existing.
        host.eval_to_string("document.getElementById('b').dataset.role = 'user';").unwrap();
        assert_eq!(
            host.eval_to_string("document.getElementById('b').getAttribute('data-role')").unwrap(),
            "user"
        );
        // Delete.
        host.eval_to_string("delete document.getElementById('b').dataset.role;").unwrap();
        assert_eq!(
            host.eval_to_string("document.getElementById('b').hasAttribute('data-role')").unwrap(),
            "false"
        );
        // Enumerate via Object.keys.
        assert_eq!(
            host.eval_to_string("Object.keys(document.getElementById('b').dataset).sort().join(',')").unwrap(),
            "fooBar,userId"
        );
    }

    #[test]
    fn inline_style_api() {
        let html = "<html><body><div id=\"box\" style=\"color: red\">x</div></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document.clone());

        // Read existing inline declaration via camelCase and getPropertyValue.
        assert_eq!(host.eval_to_string("document.getElementById('box').style.color").unwrap(), "red");
        assert_eq!(
            host.eval_to_string("document.getElementById('box').style.getPropertyValue('color')").unwrap(),
            "red"
        );
        // camelCase setter maps to kebab-case and writes the style attribute.
        host.eval_to_string("document.getElementById('box').style.backgroundColor = 'blue';").unwrap();
        assert_eq!(
            host.eval_to_string("document.getElementById('box').style.backgroundColor").unwrap(),
            "blue"
        );
        // setProperty for arbitrary props; the change is visible on the DOM
        // attribute the engine reads at layout time.
        host.eval_to_string("document.getElementById('box').style.setProperty('display', 'none');").unwrap();
        assert_eq!(
            host.eval_to_string("document.getElementById('box').getAttribute('style')").unwrap(),
            "color: red; background-color: blue; display: none"
        );
        // removeProperty returns the old value and drops the declaration.
        assert_eq!(
            host.eval_to_string("document.getElementById('box').style.removeProperty('display')").unwrap(),
            "none"
        );
        assert_eq!(host.eval_to_string("document.getElementById('box').style.display").unwrap(), "");
        // cssText round-trips.
        host.eval_to_string("document.getElementById('box').style.cssText = 'width: 10px; height: 20px';").unwrap();
        assert_eq!(host.eval_to_string("document.getElementById('box').style.width").unwrap(), "10px");
        assert_eq!(
            host.eval_to_string("document.getElementById('box').getAttribute('style')").unwrap(),
            "width: 10px; height: 20px"
        );
    }

    #[test]
    fn event_capture_phase_ordering() {
        let html = "<html><body><div id=\"outer\"><button id=\"btn\">x</button></div>\
                    <p id=\"log\"></p></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document.clone());

        // Capture listener on the ancestor fires BEFORE the target's listener;
        // a bubble listener on the ancestor fires AFTER.
        host.eval_to_string(
            "function log(s){ document.getElementById('log').textContent += s; } \
             document.getElementById('outer').addEventListener('click', function(){ log('C'); }, true); \
             document.getElementById('btn').addEventListener('click', function(){ log('T'); }); \
             document.getElementById('outer').addEventListener('click', function(){ log('B'); }, false);",
        )
        .unwrap();

        let btn = get_element_by_id(Some(document.clone()), &"btn".to_string()).unwrap();
        host.dispatch_event(btn, "click");
        assert_eq!(
            host.eval_to_string("document.getElementById('log').textContent").unwrap(),
            "CTB"
        );

        // {capture:true} option form works, and a capture-phase stopPropagation
        // suppresses the target + bubble listeners.
        host.eval_to_string(
            "document.getElementById('log').textContent=''; \
             document.getElementById('outer').addEventListener('click', function(e){ e.stopPropagation(); log('X'); }, {capture:true});",
        )
        .unwrap();
        let btn2 = get_element_by_id(Some(document), &"btn".to_string()).unwrap();
        host.dispatch_event(btn2, "click");
        // The pre-existing capture listener 'C' runs, then the new capturing
        // 'X' stops propagation — no 'T'/'B'.
        assert_eq!(
            host.eval_to_string("document.getElementById('log').textContent").unwrap(),
            "CX"
        );
    }

    #[test]
    fn wrapper_identity_is_stable() {
        let html = "<html><body><div id=\"a\"><span id=\"b\">x</span></div></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document);

        // Same node, separate lookups -> identical wrapper object.
        assert_eq!(
            host.eval_to_string("document.getElementById('a') === document.getElementById('a')").unwrap(),
            "true"
        );
        // Identity holds across different access paths (query vs navigation).
        assert_eq!(
            host.eval_to_string(
                "document.getElementById('b') === document.querySelector('#a').firstChild"
            )
            .unwrap(),
            "true"
        );
        // Distinct nodes are not equal.
        assert_eq!(
            host.eval_to_string("document.getElementById('a') === document.getElementById('b')").unwrap(),
            "false"
        );
        // A wrapper can be used as a Set/Map-style key across lookups.
        assert_eq!(
            host.eval_to_string(
                "var seen = document.getElementById('a'); \
                 seen.dataset_marker = 1; \
                 document.getElementById('a').dataset_marker"
            )
            .unwrap(),
            "1"
        );
    }

    #[test]
    fn navigation_accessors() {
        let html =
            "<html><body><ul id=\"list\"><li id=\"a\">A</li><li id=\"b\">B</li></ul></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document);

        assert_eq!(
            host.eval_to_string("document.getElementById('a').parentNode.id").unwrap(),
            "list"
        );
        assert_eq!(
            host.eval_to_string("document.getElementById('a').nextSibling.id").unwrap(),
            "b"
        );
        assert_eq!(
            host.eval_to_string("document.getElementById('b').previousSibling.id").unwrap(),
            "a"
        );
        assert_eq!(
            host.eval_to_string("document.getElementById('list').children.length").unwrap(),
            "2"
        );
        assert_eq!(
            host.eval_to_string("document.getElementById('list').children[1].id").unwrap(),
            "b"
        );
    }

    #[test]
    fn console_and_timers_via_event_loop() {
        let html = "<html><body><p id=\"out\"></p></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document);

        host.eval_to_string("console.log('hello', 42); console.warn('w');").unwrap();
        assert_eq!(host.take_console_log(), vec!["hello 42".to_string(), "w".to_string()]);

        // Timers do not fire until the loop is pumped.
        host.eval_to_string(
            "setTimeout(function() { \
                 document.getElementById('out').textContent += 'A'; \
                 setTimeout(function() { document.getElementById('out').textContent += 'B'; }, 0); \
             }, 0);",
        )
        .unwrap();
        assert_eq!(host.eval_to_string("document.getElementById('out').textContent").unwrap(), "");
        host.run_pending(100);
        // Nested setTimeout also ran, in order.
        assert_eq!(host.eval_to_string("document.getElementById('out').textContent").unwrap(), "AB");

        // clearTimeout cancels a pending timer.
        host.eval_to_string(
            "var id = setTimeout(function() { \
                 document.getElementById('out').textContent += 'X'; }, 5); \
             clearTimeout(id);",
        )
        .unwrap();
        host.run_pending(100);
        assert_eq!(host.eval_to_string("document.getElementById('out').textContent").unwrap(), "AB");
    }

    /// A deterministic FetchEngine for tests: sends a canned response for the
    /// URL immediately, so pump_fetches settles it on the next drain.
    struct MockFetch {
        status: u16,
        body: String,
    }
    impl FetchEngine for MockFetch {
        fn start(&self, req: FetchRequest) -> std::sync::mpsc::Receiver<FetchResponse> {
            let (tx, rx) = std::sync::mpsc::channel();
            tx.send(FetchResponse {
                ok: (200..300).contains(&self.status),
                status: self.status,
                status_text: "OK".to_string(),
                url: req.url,
                body: self.body.clone(),
                error: None,
            })
            .unwrap();
            rx
        }
    }

    #[test]
    fn fetch_resolves_via_engine_and_mutates_dom() {
        let html = "<html><body><ul id=\"out\"></ul></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_fetch_engine(Box::new(MockFetch {
            status: 200,
            body: r#"{"items":["a","b","c"]}"#.to_string(),
        }));
        host.set_document(document);

        host.eval_to_string(
            "globalThis.done = ''; \
             fetch('/data.json') \
                .then(function(r) { globalThis.status = r.status; globalThis.ok = r.ok; return r.json(); }) \
                .then(function(data) { \
                    var ul = document.getElementById('out'); \
                    for (var i = 0; i < data.items.length; i++) { \
                        var li = document.createElement('li'); li.textContent = data.items[i]; ul.appendChild(li); \
                    } \
                    globalThis.done = 'ok:' + data.items.length; \
                });",
        )
        .unwrap();

        // Nothing resolved until the loop is pumped.
        assert_eq!(host.eval_to_string("globalThis.done").unwrap(), "");
        assert!(host.has_pending_fetches());

        host.run_initial_load(100);

        assert_eq!(host.eval_to_string("globalThis.status").unwrap(), "200");
        assert_eq!(host.eval_to_string("globalThis.ok").unwrap(), "true");
        assert_eq!(host.eval_to_string("globalThis.done").unwrap(), "ok:3");
        assert_eq!(host.eval_to_string("document.getElementById('out').textContent").unwrap(), "abc");
        assert!(!host.has_pending_fetches());
    }

    #[test]
    fn xhr_async_delivers_response_and_fires_handlers() {
        let html = "<html><body><p id=\"out\"></p></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_fetch_engine(Box::new(MockFetch {
            status: 201,
            body: "hello-xhr".to_string(),
        }));
        host.set_document(document);

        host.eval_to_string(
            "globalThis.log = ''; \
             var x = new XMLHttpRequest(); \
             x.open('GET', '/thing'); \
             globalThis.opened = x.readyState; \
             x.onload = function() { \
                 globalThis.log = x.readyState + ':' + x.status + ':' + x.responseText; \
                 document.getElementById('out').textContent = x.responseText; \
             }; \
             x.send();",
        )
        .unwrap();

        // readyState is OPENED (1) after open(); response not delivered yet.
        assert_eq!(host.eval_to_string("globalThis.opened").unwrap(), "1");
        assert_eq!(host.eval_to_string("globalThis.log").unwrap(), "");
        assert!(host.has_pending_fetches());

        host.run_initial_load(100);

        // onload fired with DONE (4), status and body populated.
        assert_eq!(host.eval_to_string("globalThis.log").unwrap(), "4:201:hello-xhr");
        assert_eq!(host.eval_to_string("document.getElementById('out').textContent").unwrap(), "hello-xhr");
        assert!(!host.has_pending_fetches());
    }

    /// Captures the last request it received, for asserting method/headers/body.
    struct CapturingFetch {
        last: std::sync::Arc<std::sync::Mutex<Option<(String, Vec<(String, String)>, Option<String>)>>>,
    }
    impl FetchEngine for CapturingFetch {
        fn start(&self, req: FetchRequest) -> std::sync::mpsc::Receiver<FetchResponse> {
            *self.last.lock().unwrap() = Some((req.method.clone(), req.headers.clone(), req.body.clone()));
            let (tx, rx) = std::sync::mpsc::channel();
            tx.send(FetchResponse {
                ok: true,
                status: 200,
                status_text: "OK".to_string(),
                url: req.url,
                body: "{}".to_string(),
                error: None,
            })
            .unwrap();
            rx
        }
    }

    #[test]
    fn fetch_and_xhr_forward_headers_and_body() {
        let html = "<html><body></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        let last = std::sync::Arc::new(std::sync::Mutex::new(None));
        host.set_fetch_engine(Box::new(CapturingFetch { last: last.clone() }));
        host.set_document(document);

        // fetch with method/headers/body.
        host.eval_to_string(
            "fetch('/p', {method:'POST', headers:{'Content-Type':'application/json','X-Test':'1'}, body:'{\"a\":1}'});",
        )
        .unwrap();
        host.run_initial_load(100);
        {
            let (method, headers, body) = last.lock().unwrap().clone().unwrap();
            assert_eq!(method, "POST");
            assert_eq!(body.as_deref(), Some("{\"a\":1}"));
            assert!(headers.iter().any(|(k, v)| k == "Content-Type" && v == "application/json"));
            assert!(headers.iter().any(|(k, v)| k == "X-Test" && v == "1"));
        }

        // XHR setRequestHeader.
        host.eval_to_string(
            "var x=new XMLHttpRequest(); x.open('GET','/q'); x.setRequestHeader('Authorization','Bearer t'); x.send();",
        )
        .unwrap();
        host.run_initial_load(100);
        let (method, headers, _body) = last.lock().unwrap().clone().unwrap();
        assert_eq!(method, "GET");
        assert!(headers.iter().any(|(k, v)| k == "Authorization" && v == "Bearer t"));
    }

    #[test]
    fn fetch_without_engine_rejects() {
        let html = "<html><body></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document);

        host.eval_to_string(
            "globalThis.err = ''; \
             fetch('/x').catch(function(e) { globalThis.err = 'rejected'; });",
        )
        .unwrap();
        host.run_initial_load(100);
        assert_eq!(host.eval_to_string("globalThis.err").unwrap(), "rejected");
    }

    #[test]
    fn run_frame_drives_raf_animation_one_step_per_frame() {
        let html = "<html><body><p id=\"out\"></p></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document);

        // A self-perpetuating rAF loop that advances a counter each frame.
        host.eval_to_string(
            "var frames = 0; \
             function step(ts){ frames++; document.getElementById('out').textContent = String(frames); if (frames < 5) requestAnimationFrame(step); } \
             requestAnimationFrame(step);",
        )
        .unwrap();
        assert_eq!(host.eval_to_string("frames").unwrap(), "0");
        assert!(host.has_pending_timers(), "a rAF is queued");

        // Each run_frame advances the loop by exactly one step.
        for expected in 1..=5 {
            host.run_frame(16, 64);
            assert_eq!(host.eval_to_string("frames").unwrap(), expected.to_string());
        }
        // After 5 frames the loop stopped requesting; no more pending work.
        host.run_frame(16, 64);
        assert_eq!(host.eval_to_string("frames").unwrap(), "5");
        assert!(!host.has_pending_timers(), "the rAF loop has finished");
        assert_eq!(host.eval_to_string("document.getElementById('out').textContent").unwrap(), "5");
    }

    #[test]
    fn initial_load_does_not_spin_intervals() {
        let html = "<html><body><p id=\"out\"></p></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document);

        // At initial load, a setInterval must fire at most once (no spinning
        // to the fire cap), while setTimeout(0) chains still resolve.
        host.eval_to_string(
            "var n = 0; \
             setInterval(function() { n++; document.getElementById('out').textContent += 'i'; }, 10); \
             setTimeout(function() { document.getElementById('out').textContent += 'T'; }, 0);",
        )
        .unwrap();
        host.run_initial_load(1000);
        assert_eq!(host.eval_to_string("n").unwrap(), "1", "interval fired more than once");
        assert_eq!(host.eval_to_string("document.getElementById('out').textContent").unwrap(), "Ti");

        // By contrast, run_pending keeps the interval loop going (bounded).
        host.eval_to_string("n = 0;").unwrap();
        host.eval_to_string(
            "setInterval(function() { n++; }, 10);",
        )
        .unwrap();
        host.run_pending(5);
        assert!(host.eval_to_string("n").unwrap().parse::<i32>().unwrap() >= 2, "run_pending should keep ticking");
    }

    #[test]
    fn event_listeners_dispatch_and_bubble() {
        let html = "<html><body><div id=\"outer\"><button id=\"btn\">x</button></div>\
                    <p id=\"log\"></p></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document.clone());

        // A click handler on the button appends to a log; a bubbling handler
        // on the ancestor also fires.
        host.eval_to_string(
            "document.getElementById('btn').addEventListener('click', function(e) { \
                 document.getElementById('log').textContent += 'B'; \
             }); \
             document.getElementById('outer').addEventListener('click', function(e) { \
                 document.getElementById('log').textContent += 'O'; \
             });",
        )
        .unwrap();

        // Dispatch from JS: target handler then bubbling ancestor handler.
        host.eval_to_string("document.getElementById('btn').dispatchEvent({type:'click'});")
            .unwrap();
        assert_eq!(host.eval_to_string("document.getElementById('log').textContent").unwrap(), "BO");

        // Dispatch from the runtime side returns default-not-prevented.
        let btn = get_element_by_id(Some(document.clone()), &"btn".to_string()).unwrap();
        assert!(host.dispatch_event(btn.clone(), "click"));
        assert_eq!(host.eval_to_string("document.getElementById('log').textContent").unwrap(), "BOBO");

        // stopPropagation halts the bubble; preventDefault flips the return.
        host.eval_to_string(
            "document.getElementById('log').textContent = ''; \
             document.getElementById('btn').addEventListener('click', function(e) { \
                 e.stopPropagation(); e.preventDefault(); \
             });",
        )
        .unwrap();
        assert!(!host.dispatch_event(btn, "click"));
        // Only the button's two handlers ran (B twice); the ancestor 'O' was
        // suppressed by stopPropagation.
        assert_eq!(host.eval_to_string("document.getElementById('log').textContent").unwrap(), "B");
    }

    #[test]
    fn class_list_add_remove_toggle_contains() {
        let html = "<html><body><div id=\"box\" class=\"a b\">x</div></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        let mut host = ScriptHost::new();
        host.set_document(document);

        assert_eq!(
            host.eval_to_string("document.getElementById('box').classList.contains('a')").unwrap(),
            "true"
        );
        assert_eq!(
            host.eval_to_string("document.getElementById('box').classList.contains('z')").unwrap(),
            "false"
        );
        // add is idempotent and appends new tokens.
        host.eval_to_string("document.getElementById('box').classList.add('a', 'c');").unwrap();
        assert_eq!(
            host.eval_to_string("document.getElementById('box').className").unwrap(),
            "a b c"
        );
        // remove drops the token.
        host.eval_to_string("document.getElementById('box').classList.remove('b');").unwrap();
        assert_eq!(
            host.eval_to_string("document.getElementById('box').className").unwrap(),
            "a c"
        );
        // toggle returns the resulting membership.
        assert_eq!(
            host.eval_to_string("document.getElementById('box').classList.toggle('a')").unwrap(),
            "false"
        );
        assert_eq!(
            host.eval_to_string("document.getElementById('box').classList.toggle('d')").unwrap(),
            "true"
        );
        assert_eq!(
            host.eval_to_string("document.getElementById('box').className").unwrap(),
            "c d"
        );
        // toggle with force keeps the forced state.
        assert_eq!(
            host.eval_to_string("document.getElementById('box').classList.toggle('c', true)").unwrap(),
            "true"
        );
        assert_eq!(
            host.eval_to_string("document.getElementById('box').className").unwrap(),
            "c d"
        );
    }
}
