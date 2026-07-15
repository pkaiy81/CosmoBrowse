//! JavaScript engine integration (plan Phase 3). Wraps boa_engine so the
//! rest of the browser can evaluate scripts without depending on Boa's API
//! surface directly — DOM bindings and the event loop layer on top of this.

use boa_engine::{
    js_string,
    object::{JsObject, ObjectInitializer},
    property::{Attribute, PropertyDescriptor},
    Context, JsData, JsResult, JsValue, NativeFunction, Source,
};
use boa_engine::gc::{Finalize, Trace};
use boa_engine::object::builtins::JsArray;
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
}

/// A scheduled `setTimeout`/`setInterval` callback awaiting its turn.
struct Timer {
    id: u32,
    callback: JsObject,
    /// Virtual fire time (accumulated delay), used only to order due timers.
    due: u64,
    /// Repeat interval in ms for `setInterval`; `None` for one-shot timers.
    interval: Option<u64>,
}

thread_local! {
    /// Event listeners keyed by DOM node identity (`Rc::as_ptr`). The DOM
    /// nodes themselves stay outside Boa's GC (plan D5), so listeners live
    /// here rather than on the node; cleared on navigation via `clear_state`.
    static LISTENERS: RefCell<std::collections::HashMap<usize, Vec<Listener>>> =
        RefCell::new(std::collections::HashMap::new());

    /// Lines emitted by `console.*`, in order. Drained by the runtime/tests.
    static CONSOLE_LOG: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };

    /// Pending timers (setTimeout/setInterval) and the next timer id.
    static TIMERS: RefCell<Vec<Timer>> = const { RefCell::new(Vec::new()) };
    static NEXT_TIMER_ID: std::cell::Cell<u32> = const { std::cell::Cell::new(1) };
    /// Monotonic virtual clock advanced as timers fire (see run_pending).
    static VIRTUAL_CLOCK: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };

    /// The document URL exposed as `location`. Read by the location accessors;
    /// updated by `ScriptHost::set_location` and by assigning `location.href`.
    static LOCATION_HREF: RefCell<String> = RefCell::new(String::from("about:blank"));

    /// `localStorage` backing store: insertion-ordered key/value pairs (order
    /// matters for `key(n)`). Seeded/snapshotted by the runtime per origin.
    static LOCAL_STORAGE: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };

    /// Messages posted via `window.parent.postMessage`, JSON-serialized in
    /// order. The runtime drains these (e.g. to handle cosmobrowse:navigate
    /// from the injected link-interception script).
    static POSTED_MESSAGES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

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

/// Wrap a DOM node as an `Element` JsObject exposing live accessors
/// (textContent, id, className, tagName) and attribute methods.
fn make_element(node: Rc<RefCell<Node>>, context: &mut Context) -> JsObject {
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
    let mut cb = child.borrow_mut();
    cb.set_parent(Weak::new());
    cb.set_previous_sibling(Weak::new());
    cb.set_next_sibling(None);
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
    LISTENERS.with(|m| {
        m.borrow_mut()
            .entry(node_key(&node))
            .or_default()
            .push(Listener { event_type, callback: cb });
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
    let not_prevented = run_dispatch(node, &event_type, c);
    Ok(JsValue::from(not_prevented))
}

/// Build an `Event` JsObject carrying `type`, `target`, propagation flags, and
/// the `preventDefault` / `stopPropagation` methods.
fn make_event(target: &Rc<RefCell<Node>>, event_type: &str, ctx: &mut Context) -> JsObject {
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
    obj
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

/// Dispatch `event_type` at `target` and bubble up its ancestor chain. Returns
/// `true` if `preventDefault` was NOT called (i.e. the default action runs).
fn run_dispatch(target: Rc<RefCell<Node>>, event_type: &str, ctx: &mut Context) -> bool {
    let event = make_event(&target, event_type, ctx);

    // Bubble order: target first, then each ancestor.
    let mut chain = vec![target.clone()];
    let mut cur = target.borrow().parent().upgrade();
    while let Some(p) = cur {
        chain.push(p.clone());
        cur = p.borrow().parent().upgrade();
    }

    'outer: for node in chain {
        let callbacks: Vec<JsObject> = LISTENERS.with(|m| {
            m.borrow()
                .get(&node_key(&node))
                .map(|v| {
                    v.iter()
                        .filter(|l| l.event_type == event_type)
                        .map(|l| l.callback.clone())
                        .collect()
                })
                .unwrap_or_default()
        });
        for cb in callbacks {
            let this = JsValue::from(make_element(node.clone(), ctx));
            let _ = cb.call(&this, &[JsValue::from(event.clone())], ctx);
            let stop = event
                .downcast_ref::<EventFlags>()
                .map(|f| f.stop_propagation.get())
                .unwrap_or(false);
            if stop {
                break 'outer;
            }
        }
    }

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
    Ok(node_or_null(query_selector(node, &sel), c))
}

fn elem_query_selector_all(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let Some(node) = handle_node(this) else {
        return Ok(JsValue::from(JsArray::new(c)));
    };
    let sel = a.first().cloned().unwrap_or_default().to_string(c)?.to_std_string_escaped();
    let arr = JsArray::new(c);
    for n in query_selector_all(node, &sel) {
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

fn serialize_node(node: &Rc<RefCell<Node>>, out: &mut String) {
    match node.borrow().kind() {
        NodeKind::Text(t) => out.push_str(&t),
        NodeKind::Element(e) => {
            let tag = e.tag_name().to_string();
            out.push('<');
            out.push_str(&tag);
            for attr in e.attributes() {
                out.push(' ');
                out.push_str(&attr.name());
                out.push_str("=\"");
                out.push_str(&attr.value());
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
        return Ok(JsValue::undefined());
    }
    node.borrow_mut()
        .set_first_child(Some(Rc::new(RefCell::new(Node::new(NodeKind::Text(text))))));
    Ok(JsValue::undefined())
}

thread_local! {
    /// The document currently exposed to script. The DOM stays a
    /// `Rc<RefCell<Node>>` outside Boa's GC (plan D5); native functions read
    /// it from here rather than capturing it (Boa closures require Trace,
    /// which the Rc DOM does not implement). The engine is single-threaded
    /// per page, so a thread-local is sufficient — the same transitional
    /// pattern used for font metrics and the styling viewport.
    static SCRIPT_DOM: RefCell<Option<Rc<RefCell<Node>>>> = const { RefCell::new(None) };
}

/// A per-page JavaScript execution context.
pub struct ScriptHost {
    context: Context,
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
        };
        host.install_dom_globals();
        host
    }

    /// Expose the given document root to script as `document`. Clears any
    /// event listeners left over from a previous document (plan D5: the
    /// registry is per-page and reset on navigation).
    pub fn set_document(&mut self, root: Rc<RefCell<Node>>) {
        LISTENERS.with(|m| m.borrow_mut().clear());
        TIMERS.with(|t| t.borrow_mut().clear());
        VIRTUAL_CLOCK.with(|c| c.set(0));
        SCRIPT_DOM.with(|d| *d.borrow_mut() = Some(root));
    }

    /// Fire an event of `event_type` at `target`, bubbling up its ancestor
    /// chain. Returns `true` if the default action should run (i.e.
    /// `preventDefault` was not called). Used by the runtime to route real
    /// input events (click/input/submit) into script.
    pub fn dispatch_event(&mut self, target: Rc<RefCell<Node>>, event_type: &str) -> bool {
        run_dispatch(target, event_type, &mut self.context)
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

        // Timers: setTimeout/setInterval/clearTimeout/clearInterval.
        for (name, f) in [
            ("setTimeout", set_timeout as fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>),
            ("setInterval", set_interval),
            ("clearTimeout", clear_timer),
            ("clearInterval", clear_timer),
        ] {
            let func = NativeFunction::from_fn_ptr(f).to_js_function(&self.context.realm().clone());
            self.context
                .register_global_property(js_string!(name), func, Attribute::all())
                .expect("register timer");
        }

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
        POSTED_MESSAGES.with(|m| std::mem::take(&mut *m.borrow_mut()))
    }

    /// Set the URL exposed to script as `location`.
    pub fn set_location(&mut self, href: &str) {
        LOCATION_HREF.with(|h| *h.borrow_mut() = href.to_string());
    }

    /// Snapshot the current `localStorage` contents (for per-origin
    /// persistence by the runtime).
    pub fn local_storage_entries(&self) -> Vec<(String, String)> {
        LOCAL_STORAGE.with(|s| s.borrow().clone())
    }

    /// Replace `localStorage` with the given entries (seed from persistence).
    pub fn set_local_storage_entries(&mut self, entries: Vec<(String, String)>) {
        LOCAL_STORAGE.with(|s| *s.borrow_mut() = entries);
    }

    /// Evaluate a script and return its completion value rendered as a
    /// display string (for smoke tests / diagnostics). Errors are returned
    /// as `Err(message)` rather than panicking.
    pub fn eval_to_string(&mut self, source: &str) -> Result<String, String> {
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
        CONSOLE_LOG.with(|l| std::mem::take(&mut *l.borrow_mut()))
    }

    /// Run the event loop until it settles: Boa's promise/microtask jobs plus
    /// all due timers. Timers fire in due order on a virtual clock (delays
    /// order them but do not block), so `setTimeout(f, 0)` chains resolve.
    /// `max_timer_fires` bounds runaway `setInterval` loops.
    pub fn run_pending(&mut self, max_timer_fires: usize) {
        self.context.run_jobs();
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
            let _ = timer
                .callback
                .call(&JsValue::undefined(), &[], &mut self.context);
            // Reschedule intervals relative to the virtual clock.
            if let Some(iv) = timer.interval {
                let due = VIRTUAL_CLOCK.with(|c| c.get()) + iv.max(1);
                TIMERS.with(|t| {
                    t.borrow_mut().push(Timer {
                        id: timer.id,
                        callback: timer.callback,
                        due,
                        interval: Some(iv),
                    })
                });
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
    LISTENERS.with(|m| {
        m.borrow_mut()
            .entry(node_key(&root))
            .or_default()
            .push(Listener { event_type, callback: cb });
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
    Ok(JsValue::from(run_dispatch(root, &event_type, c)))
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
