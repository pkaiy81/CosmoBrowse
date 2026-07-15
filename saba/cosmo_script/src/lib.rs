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
    collect_text, get_element_by_id, query_selector, query_selector_all,
};
use cosmo_engine::renderer::dom::node::{Element, Node, NodeKind};
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
