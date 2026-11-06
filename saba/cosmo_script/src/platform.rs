//! The native half of the web platform: what `bootstrap.js` needs from the
//! engine, and running a document's scripts.

use super::*;
use boa_engine::module::{ModuleLoader, Referrer};
use boa_engine::object::builtins::JsArray;
use boa_engine::{JsError, JsNativeError, JsString, Module};
use cosmo_engine::renderer::css::media::{parse_media_query_list, MediaContext};
use cosmo_engine::renderer::css::token::CssTokenizer;
use cosmo_engine::renderer::dom::node::Namespace;
use std::path::Path;

/// What script can ask of layout. The host answers from the page's current
/// layout, which it keeps as fresh as it can afford to.
pub trait LayoutProvider: 'static {
    /// The element's border box, in page coordinates: x, y, width, height.
    fn rect(&self, node: &Rc<RefCell<Node>>) -> Option<[f64; 4]>;
    /// The element's computed style, as CSS property names and values.
    fn computed_style(&self, node: &Rc<RefCell<Node>>) -> Vec<(String, String)>;
    /// The viewport's width and height.
    fn viewport(&self) -> (f64, f64);
}

/// A key for the wrapper's prototype: its tag for an HTML element.
fn proto_key(node: &Rc<RefCell<Node>>) -> String {
    match node.borrow().kind {
        NodeKind::Document => "HTMLDocument".to_string(),
        NodeKind::Text(_) => "Text".to_string(),
        NodeKind::Comment(_) => "Comment".to_string(),
        NodeKind::Doctype { .. } => "DocumentType".to_string(),
        NodeKind::Element(ref element) => match element.namespace() {
            Namespace::Html => element.tag_name().to_string(),
            Namespace::Svg => "SVGElement".to_string(),
            Namespace::MathMl => "MathMLElement".to_string(),
        },
    }
}

/// The prototype a wrapper for this node gets.
pub(crate) fn proto_for(node: &Rc<RefCell<Node>>) -> Option<JsObject> {
    let key = proto_key(node);
    PROTOS.with(|protos| {
        let protos = protos.borrow();
        protos
            .get(&key)
            .or_else(|| {
                let is_html_element = matches!(
                    node.borrow().kind,
                    NodeKind::Element(ref e) if e.namespace() == Namespace::Html
                );
                is_html_element.then(|| protos.get("HTMLElement")).flatten()
            })
            .or_else(|| protos.get("Node"))
            .cloned()
    })
}

fn string_arg(args: &[JsValue], index: usize, ctx: &mut Context) -> JsResult<String> {
    Ok(args
        .get(index)
        .cloned()
        .unwrap_or_default()
        .to_string(ctx)?
        .to_std_string_escaped())
}

fn js_str(text: &str) -> JsValue {
    JsValue::from(js_string!(text))
}

fn set_proto(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let name = string_arg(a, 0, c)?;
    if let Some(proto) = a.get(1).and_then(|v| v.as_object()).cloned() {
        PROTOS.with(|protos| protos.borrow_mut().insert(name, proto));
    }
    Ok(JsValue::undefined())
}

fn set_window_dispatcher(_t: &JsValue, a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    WINDOW_DISPATCHER.with(|slot| *slot.borrow_mut() = a.first().and_then(|v| v.as_object()).cloned());
    Ok(JsValue::undefined())
}

fn set_lifecycle(_t: &JsValue, a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    LIFECYCLE.with(|slot| *slot.borrow_mut() = a.first().and_then(|v| v.as_object()).cloned());
    Ok(JsValue::undefined())
}

fn new_event(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let event_type = string_arg(a, 0, c)?;
    let bubbles = a.get(1).is_some_and(|v| v.to_boolean());
    let cancelable = a.get(2).is_some_and(|v| v.to_boolean());
    let proto = a.get(3).and_then(|v| v.as_object()).cloned();
    Ok(JsValue::from(build_event(&event_type, bubbles, cancelable, proto, c)))
}

fn create_element(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let tag = string_arg(a, 0, c)?;
    let node = Rc::new(RefCell::new(Node::new(NodeKind::Element(Element::new(&tag, Vec::new())))));
    Ok(JsValue::from(make_element(node, c)))
}

fn create_element_ns(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let namespace = string_arg(a, 0, c)?;
    let tag = string_arg(a, 1, c)?;
    let namespace = if namespace.contains("svg") {
        Namespace::Svg
    } else if namespace.contains("MathML") {
        Namespace::MathMl
    } else {
        Namespace::Html
    };
    let node = Rc::new(RefCell::new(Node::new(NodeKind::Element(Element::new_in(
        &tag,
        Vec::new(),
        namespace,
    )))));
    Ok(JsValue::from(make_element(node, c)))
}

fn create_comment(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let text = string_arg(a, 0, c)?;
    let node = Rc::new(RefCell::new(Node::new(NodeKind::Comment(text))));
    Ok(JsValue::from(make_element(node, c)))
}

fn namespace_of(_t: &JsValue, a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    let namespace = a
        .first()
        .and_then(handle_node)
        .and_then(|n| match n.borrow().kind {
            NodeKind::Element(ref e) => Some(e.namespace()),
            _ => None,
        });
    Ok(match namespace {
        Some(Namespace::Html) => js_str("http://www.w3.org/1999/xhtml"),
        Some(Namespace::Svg) => js_str("http://www.w3.org/2000/svg"),
        Some(Namespace::MathMl) => js_str("http://www.w3.org/1998/Math/MathML"),
        None => JsValue::null(),
    })
}

fn attributes(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let list = JsArray::new(c);
    if let Some(node) = a.first().and_then(handle_node) {
        if let NodeKind::Element(ref element) = node.borrow().kind {
            for attribute in element.attributes() {
                let pair = JsArray::new(c);
                pair.push(js_str(&attribute.name()), c)?;
                pair.push(js_str(&attribute.value()), c)?;
                list.push(pair, c)?;
            }
        }
    }
    Ok(list.into())
}

fn outer_html(_t: &JsValue, a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    let mut out = String::new();
    if let Some(node) = a.first().and_then(handle_node) {
        serialize_node(&node, &mut out);
    }
    Ok(js_str(&out))
}

fn inner_html_of(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let this = a.first().cloned().unwrap_or_default();
    element_get_inner_html(&this, &[], c)
}

fn set_inner_html(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let this = a.first().cloned().unwrap_or_default();
    element_set_inner_html(&this, &a[1.min(a.len())..], c)
}

/// Scripts in parsed markup are not run, other than a document's own.
fn mark_scripts_started(node: &Rc<RefCell<Node>>) {
    if node.borrow().element_tag_name().as_deref() == Some("script") {
        STARTED_SCRIPTS.with(|started| started.borrow_mut().insert(node_key(node)));
    }
    let mut child = node.borrow().first_child();
    while let Some(c) = child {
        mark_scripts_started(&c);
        child = c.borrow().next_sibling();
    }
}

fn parse_fragment(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let context = string_arg(a, 0, c)?;
    let html = string_arg(a, 1, c)?;
    let list = JsArray::new(c);
    for node in HtmlParser::parse_fragment(&context, html) {
        cosmo_engine::renderer::dom::api::detach_node(&node);
        mark_scripts_started(&node);
        list.push(make_element(node, c), c)?;
    }
    Ok(list.into())
}

fn parse_document(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let html = string_arg(a, 0, c)?;
    let window = HtmlParser::new(cosmo_engine::renderer::html::token::HtmlTokenizer::new(html))
        .construct_tree();
    let document = window.borrow().document();
    mark_scripts_started(&document);
    // Keep it alive with the wrapper: its nodes only hold weak links upward.
    Ok(JsValue::from(make_element(document, c)))
}

fn parse_url(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let input = string_arg(a, 0, c)?;
    let base = match a.get(1) {
        Some(value) if !value.is_undefined() => Some(value.to_string(c)?.to_std_string_escaped()),
        _ => None,
    };
    let parsed = match &base {
        Some(base) => url::Url::parse(base).ok().and_then(|b| b.join(&input).ok()),
        None => url::Url::parse(&input).ok(),
    };
    let Some(url) = parsed else {
        return Ok(JsValue::null());
    };
    let port = url.port().map(|p| p.to_string()).unwrap_or_default();
    let hostname = url.host_str().unwrap_or("").to_string();
    let host = if port.is_empty() {
        hostname.clone()
    } else {
        format!("{hostname}:{port}")
    };
    let origin = url.origin().ascii_serialization();
    let search = url.query().filter(|q| !q.is_empty()).map(|q| format!("?{q}")).unwrap_or_default();
    let hash = url.fragment().filter(|f| !f.is_empty()).map(|f| format!("#{f}")).unwrap_or_default();
    let object = ObjectInitializer::new(c)
        .property(js_string!("href"), js_str(url.as_str()), Attribute::all())
        .property(js_string!("origin"), js_str(&origin), Attribute::all())
        .property(js_string!("protocol"), js_str(&format!("{}:", url.scheme())), Attribute::all())
        .property(js_string!("username"), js_str(url.username()), Attribute::all())
        .property(js_string!("password"), js_str(url.password().unwrap_or("")), Attribute::all())
        .property(js_string!("host"), js_str(&host), Attribute::all())
        .property(js_string!("hostname"), js_str(&hostname), Attribute::all())
        .property(js_string!("port"), js_str(&port), Attribute::all())
        .property(js_string!("pathname"), js_str(url.path()), Attribute::all())
        .property(js_string!("search"), js_str(&search), Attribute::all())
        .property(js_string!("hash"), js_str(&hash), Attribute::all())
        .build();
    Ok(object.into())
}

fn rect_of(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let Some(node) = a.first().and_then(handle_node) else {
        return Ok(JsValue::null());
    };
    let rect = LAYOUT_PROVIDER.with(|p| p.borrow().as_ref().and_then(|p| p.rect(&node)));
    let Some(rect) = rect else {
        return Ok(JsValue::null());
    };
    let list = JsArray::new(c);
    for value in rect {
        list.push(JsValue::from(value), c)?;
    }
    Ok(list.into())
}

fn computed_style(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let Some(node) = a.first().and_then(handle_node) else {
        return Ok(JsValue::null());
    };
    let values = LAYOUT_PROVIDER
        .with(|p| p.borrow().as_ref().map(|p| p.computed_style(&node)))
        .unwrap_or_default();
    let object = ObjectInitializer::new(c).build();
    for (name, value) in values {
        object.set(JsString::from(name.as_str()), js_str(&value), false, c)?;
    }
    Ok(object.into())
}

fn viewport(_t: &JsValue, _a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let (width, height) = current_viewport();
    let list = JsArray::new(c);
    list.push(JsValue::from(width), c)?;
    list.push(JsValue::from(height), c)?;
    Ok(list.into())
}

fn current_viewport() -> (f64, f64) {
    LAYOUT_PROVIDER
        .with(|p| p.borrow().as_ref().map(|p| p.viewport()))
        .unwrap_or((1024.0, 768.0))
}

fn match_media(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let query = string_arg(a, 0, c)?;
    let tokens: Vec<_> = CssTokenizer::new(query).collect();
    let (viewport_width, viewport_height) = current_viewport();
    let context = MediaContext {
        viewport_width,
        viewport_height,
        prefers_dark: std::env::var("COSMO_PREFERS_DARK").ok().as_deref() == Some("1"),
    };
    Ok(JsValue::from(parse_media_query_list(&tokens).matches(&context)))
}

fn css_supports(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let text = string_arg(a, 0, c)?;
    let css = format!("@supports {text} {{ a {{ color: red }} }}");
    let sheet = cosmo_engine::renderer::css::cssom::CssParser::new(CssTokenizer::new(css))
        .parse_stylesheet();
    Ok(JsValue::from(!sheet.rules.is_empty()))
}

/// Ask the browser to navigate. `postBody` makes it a form POST.
fn navigate(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let href = string_arg(a, 0, c)?;
    let post_body = match a.get(1) {
        Some(value) if !value.is_null_or_undefined() => {
            Some(value.to_string(c)?.to_std_string_escaped())
        }
        _ => None,
    };
    let resolved = url::Url::parse(&location_href())
        .ok()
        .and_then(|base| base.join(&href).ok())
        .map(|url| url.to_string())
        .unwrap_or(href);
    let mut message = format!(
        "{{\"type\":\"cosmobrowse:navigate\",\"href\":{}",
        json_string(&resolved)
    );
    if let Some(body) = post_body {
        message.push_str(&format!(",\"postBody\":{}", json_string(&body)));
    }
    message.push('}');
    POSTED_MESSAGES.with(|m| m.borrow_mut().push(message));
    Ok(JsValue::undefined())
}

pub(crate) fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn set_location_silently(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let href = string_arg(a, 0, c)?;
    LOCATION_HREF.with(|h| *h.borrow_mut() = href);
    Ok(JsValue::undefined())
}

fn cookie_get(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    let cookies = FETCH_ENGINE
        .with(|e| e.borrow().as_ref().map(|e| e.document_cookie()))
        .unwrap_or_default();
    Ok(js_str(&cookies))
}

fn cookie_set(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let value = string_arg(a, 0, c)?;
    FETCH_ENGINE.with(|e| {
        if let Some(engine) = e.borrow().as_ref() {
            engine.set_document_cookie(&value);
        }
    });
    Ok(JsValue::undefined())
}

fn current_script(_t: &JsValue, _a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let node = CURRENT_SCRIPT.with(|s| s.borrow().clone());
    Ok(node_or_null(node, c))
}

fn form_data(_t: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let list = JsArray::new(c);
    if let Some(form) = a.first().and_then(handle_node) {
        for (name, value) in cosmo_engine::renderer::dom::forms::form_data(&form, None) {
            let pair = JsArray::new(c);
            pair.push(js_str(&name), c)?;
            pair.push(js_str(&value), c)?;
            list.push(pair, c)?;
        }
    }
    Ok(list.into())
}

fn dom_generation_value(_t: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    Ok(JsValue::from(DOM_GENERATION.with(|g| g.get()) as f64))
}

/// `node.nodeType`.
pub(crate) fn node_get_type(this: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    let kind = handle_node(this).map(|n| match n.borrow().kind {
        NodeKind::Element(ref e) if e.tag_name() == "cosmo-fragment" => 11,
        NodeKind::Element(_) => 1,
        NodeKind::Text(_) => 3,
        NodeKind::Comment(_) => 8,
        NodeKind::Document => 9,
        NodeKind::Doctype { .. } => 10,
    });
    Ok(kind.map(JsValue::from).unwrap_or_default())
}

/// `node.nodeName`.
pub(crate) fn node_get_name(this: &JsValue, _a: &[JsValue], _c: &mut Context) -> JsResult<JsValue> {
    let name = handle_node(this).map(|n| match n.borrow().kind {
        NodeKind::Element(ref e) if e.tag_name() == "cosmo-fragment" => {
            "#document-fragment".to_string()
        }
        NodeKind::Element(ref e) if e.namespace() == Namespace::Html => e.tag_name().to_uppercase(),
        NodeKind::Element(ref e) => e.tag_name().to_string(),
        NodeKind::Text(_) => "#text".to_string(),
        NodeKind::Comment(_) => "#comment".to_string(),
        NodeKind::Document => "#document".to_string(),
        NodeKind::Doctype { ref name, .. } => name.clone(),
    });
    Ok(name.map(|n| js_str(&n)).unwrap_or_default())
}

/// `node.parentElement`: the parent, if that's an element.
pub(crate) fn node_get_parent_element(
    this: &JsValue,
    _a: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let parent = handle_node(this)
        .and_then(|n| n.borrow().parent().upgrade())
        .filter(|p| matches!(p.borrow().kind, NodeKind::Element(_)));
    Ok(node_or_null(parent, ctx))
}

pub(crate) fn element_remove_attribute(
    this: &JsValue,
    a: &[JsValue],
    c: &mut Context,
) -> JsResult<JsValue> {
    let name = string_arg(a, 0, c)?;
    if let Some(node) = handle_node(this) {
        if let NodeKind::Element(ref mut e) = node.borrow_mut().kind_mut() {
            e.remove_attribute(&name);
        }
        bump_dom_generation();
    }
    Ok(JsValue::undefined())
}

fn clone_tree(node: &Rc<RefCell<Node>>, deep: bool) -> Rc<RefCell<Node>> {
    let copy = Rc::new(RefCell::new(Node::new(node.borrow().kind.clone())));
    if deep {
        let mut child = node.borrow().first_child();
        while let Some(c) = child {
            let cloned = clone_tree(&c, true);
            append_child_node(&copy, &cloned);
            child = c.borrow().next_sibling();
        }
    }
    copy
}

pub(crate) fn node_clone(this: &JsValue, a: &[JsValue], c: &mut Context) -> JsResult<JsValue> {
    let Some(node) = handle_node(this) else {
        return Ok(JsValue::null());
    };
    let deep = a.first().is_some_and(|v| v.to_boolean());
    Ok(JsValue::from(make_element(clone_tree(&node, deep), c)))
}

fn require_function(name: &str) -> JsError {
    JsNativeError::typ()
        .with_message(format!("{name} is not available"))
        .into()
}

/// The object `bootstrap.js` builds the platform from.
pub(crate) fn make_native_object(node_base: JsObject, ctx: &mut Context) -> JsObject {
    let document_methods = ObjectInitializer::new(ctx)
        .function(NativeFunction::from_fn_ptr(dom_get_element_by_id), js_string!("getElementById"), 1)
        .function(NativeFunction::from_fn_ptr(dom_create_text_node), js_string!("createTextNode"), 1)
        .function(NativeFunction::from_fn_ptr(dom_get_elements_by_tag_name), js_string!("getElementsByTagName"), 1)
        .function(NativeFunction::from_fn_ptr(dom_get_elements_by_class_name), js_string!("getElementsByClassName"), 1)
        .build();
    let _ = require_function;
    ObjectInitializer::new(ctx)
        .property(js_string!("nodeBase"), node_base, Attribute::all())
        .property(js_string!("documentMethods"), document_methods, Attribute::all())
        .function(NativeFunction::from_fn_ptr(set_proto), js_string!("setProto"), 2)
        .function(NativeFunction::from_fn_ptr(set_window_dispatcher), js_string!("setWindowDispatcher"), 1)
        .function(NativeFunction::from_fn_ptr(set_lifecycle), js_string!("setLifecycle"), 1)
        .function(NativeFunction::from_fn_ptr(new_event), js_string!("newEvent"), 4)
        .function(NativeFunction::from_fn_ptr(create_element), js_string!("createElement"), 1)
        .function(NativeFunction::from_fn_ptr(create_element_ns), js_string!("createElementNS"), 2)
        .function(NativeFunction::from_fn_ptr(create_comment), js_string!("createComment"), 1)
        .function(NativeFunction::from_fn_ptr(namespace_of), js_string!("namespaceOf"), 1)
        .function(NativeFunction::from_fn_ptr(attributes), js_string!("attributes"), 1)
        .function(NativeFunction::from_fn_ptr(outer_html), js_string!("outerHTML"), 1)
        .function(NativeFunction::from_fn_ptr(inner_html_of), js_string!("innerHTMLOf"), 1)
        .function(NativeFunction::from_fn_ptr(set_inner_html), js_string!("setInnerHTML"), 2)
        .function(NativeFunction::from_fn_ptr(parse_fragment), js_string!("parseFragment"), 2)
        .function(NativeFunction::from_fn_ptr(parse_document), js_string!("parseDocument"), 1)
        .function(NativeFunction::from_fn_ptr(parse_url), js_string!("parseUrl"), 2)
        .function(NativeFunction::from_fn_ptr(rect_of), js_string!("rectOf"), 1)
        .function(NativeFunction::from_fn_ptr(computed_style), js_string!("computedStyle"), 1)
        .function(NativeFunction::from_fn_ptr(viewport), js_string!("viewport"), 0)
        .function(NativeFunction::from_fn_ptr(match_media), js_string!("matchMedia"), 1)
        .function(NativeFunction::from_fn_ptr(css_supports), js_string!("cssSupports"), 1)
        .function(NativeFunction::from_fn_ptr(navigate), js_string!("navigate"), 2)
        .function(NativeFunction::from_fn_ptr(set_location_silently), js_string!("setLocationSilently"), 1)
        .function(NativeFunction::from_fn_ptr(cookie_get), js_string!("cookieGet"), 0)
        .function(NativeFunction::from_fn_ptr(cookie_set), js_string!("cookieSet"), 1)
        .function(NativeFunction::from_fn_ptr(current_script), js_string!("currentScript"), 0)
        .function(NativeFunction::from_fn_ptr(form_data), js_string!("formData"), 1)
        .function(NativeFunction::from_fn_ptr(dom_generation_value), js_string!("domGeneration"), 0)
        .build()
}

// ---- Running scripts -------------------------------------------------------

/// How much a page's script text may add up to. A bound on what one page can
/// make the render thread parse, well above what real pages ship.
const MAX_DOCUMENT_SCRIPT_BYTES: usize = 32 * 1024 * 1024;
/// How long to wait for one script before going on without it.
const SCRIPT_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

fn attribute_of(node: &Rc<RefCell<Node>>, name: &str) -> Option<String> {
    match node.borrow().kind {
        NodeKind::Element(ref e) => e.get_attribute(name),
        _ => None,
    }
}

/// What kind of script an element holds, if it's one to run.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ScriptKind {
    Classic,
    Module,
}

fn script_kind(node: &Rc<RefCell<Node>>) -> Option<ScriptKind> {
    let is_svg = matches!(node.borrow().kind, NodeKind::Element(ref e) if e.namespace() != Namespace::Html);
    if is_svg {
        return None;
    }
    let declared = attribute_of(node, "type").map(|t| t.trim().to_ascii_lowercase());
    match declared.as_deref() {
        None | Some("") | Some("text/javascript") | Some("application/javascript")
        | Some("application/ecmascript") | Some("text/ecmascript") | Some("application/x-javascript") => {
            if attribute_of(node, "nomodule").is_some() {
                None
            } else {
                Some(ScriptKind::Classic)
            }
        }
        Some("module") => Some(ScriptKind::Module),
        _ => None,
    }
}

fn collect_scripts(node: &Rc<RefCell<Node>>, out: &mut Vec<Rc<RefCell<Node>>>) {
    let mut child = node.borrow().first_child();
    while let Some(c) = child {
        if c.borrow().element_tag_name().as_deref() == Some("script") {
            out.push(c.clone());
        }
        collect_scripts(&c, out);
        child = c.borrow().next_sibling();
    }
}

fn script_text(node: &Rc<RefCell<Node>>) -> String {
    let mut text = String::new();
    collect_text(node.borrow().first_child(), &mut text);
    text
}

fn start_script_fetch(url: &str) -> Option<std::sync::mpsc::Receiver<FetchResponse>> {
    FETCH_ENGINE.with(|engine| {
        engine.borrow().as_ref().map(|engine| {
            engine.start(FetchRequest {
                url: url.to_string(),
                method: "GET".to_string(),
                body: None,
                headers: Vec::new(),
                // A script may come from anywhere: that's what `<script src>` is.
                no_cors: true,
            })
        })
    })
}

fn receive_script(rx: &std::sync::mpsc::Receiver<FetchResponse>) -> Option<String> {
    let response = rx.recv_timeout(SCRIPT_FETCH_TIMEOUT).ok()?;
    (response.ok && response.error.is_none()).then_some(response.body)
}

fn log_script_error(what: &str, error: impl std::fmt::Display) {
    CONSOLE_LOG.with(|log| log.borrow_mut().push(format!("[error] {what}: {error}")));
}

fn fire_simple_event(target: &Rc<RefCell<Node>>, event_type: &str, ctx: &mut Context) {
    let event = build_event(event_type, false, false, None, ctx);
    dispatch_event_object(target.clone(), event_type, event, false, ctx);
}

impl ScriptHost {
    /// Run the document's scripts the way a browser's parser would: classic ones
    /// in order, then `defer` and modules, then what's left of `async`. Each runs
    /// on its own, so one's syntax error or exception stops only that one.
    /// Scripts run after parsing has finished, not during it, so one that looks
    /// for an element after itself finds it.
    /// https://html.spec.whatwg.org/multipage/scripting.html#the-script-element
    pub fn run_document_scripts(&mut self) {
        self.activate();
        let Some(document) = SCRIPT_DOM.with(|d| d.borrow().clone()) else {
            return;
        };
        let mut scripts = Vec::new();
        collect_scripts(&document, &mut scripts);

        // Start every fetch up front, as a browser's preload scanner would, so
        // they overlap. They still run in order.
        struct Prepared {
            node: Rc<RefCell<Node>>,
            kind: ScriptKind,
            url: Option<String>,
            pending: Option<std::sync::mpsc::Receiver<FetchResponse>>,
            deferred: bool,
            is_async: bool,
        }
        let mut prepared = Vec::new();
        for node in scripts {
            STARTED_SCRIPTS.with(|started| started.borrow_mut().insert(node_key(&node)));
            let Some(kind) = script_kind(&node) else {
                continue;
            };
            let src = attribute_of(&node, "src").filter(|s| !s.trim().is_empty());
            let pending = src.as_deref().and_then(start_script_fetch);
            if src.is_none() && script_text(&node).trim().is_empty() {
                continue;
            }
            let deferred = kind == ScriptKind::Module
                || (src.is_some() && attribute_of(&node, "defer").is_some());
            let is_async = src.is_some() && attribute_of(&node, "async").is_some();
            prepared.push(Prepared {
                node,
                kind,
                url: src,
                pending,
                deferred,
                is_async,
            });
        }
        let order = |p: &Prepared| {
            if p.is_async {
                2
            } else if p.deferred {
                1
            } else {
                0
            }
        };
        prepared.sort_by_key(order);

        let mut budget = MAX_DOCUMENT_SCRIPT_BYTES;
        for script in prepared {
            let (text, base) = match &script.pending {
                Some(rx) => match receive_script(rx) {
                    Some(text) => (text, script.url.clone()),
                    None => {
                        log_script_error(
                            "failed to load script",
                            script.url.as_deref().unwrap_or(""),
                        );
                        fire_simple_event(&script.node, "error", &mut self.context);
                        continue;
                    }
                },
                None if script.url.is_some() => continue,
                None => (script_text(&script.node), None),
            };
            if text.len() > budget {
                log_script_error("script budget exceeded, skipping", base.as_deref().unwrap_or("inline"));
                continue;
            }
            budget -= text.len();
            self.run_script(&script.node, script.kind, &text, base.as_deref());
            if script.url.is_some() {
                fire_simple_event(&script.node, "load", &mut self.context);
            }
            self.context.run_jobs();
        }
    }

    fn run_script(
        &mut self,
        node: &Rc<RefCell<Node>>,
        kind: ScriptKind,
        text: &str,
        url: Option<&str>,
    ) {
        CURRENT_SCRIPT.with(|s| *s.borrow_mut() = Some(node.clone()));
        let label = url.unwrap_or("inline script").to_string();
        match kind {
            ScriptKind::Classic => {
                if let Err(error) = self.context.eval(Source::from_bytes(text)) {
                    log_script_error(&label, error);
                }
            }
            ScriptKind::Module => {
                let base = url.map(str::to_string).unwrap_or_else(location_href);
                run_module(text, &base, &mut self.context);
            }
        }
        CURRENT_SCRIPT.with(|s| *s.borrow_mut() = None);
    }

    /// Tell the page it has finished loading.
    pub fn fire_load(&mut self) {
        self.run_lifecycle("complete");
    }

    /// Tell the page the viewport changed.
    pub fn fire_resize(&mut self) {
        self.run_lifecycle("resize");
    }

    pub(crate) fn run_lifecycle(&mut self, phase: &str) {
        self.activate();
        let callback = LIFECYCLE.with(|l| l.borrow().clone());
        if let Some(callback) = callback {
            let _ = callback.call(&JsValue::undefined(), &[js_str(phase)], &mut self.context);
        }
    }

    pub fn set_layout_provider(&mut self, provider: Box<dyn LayoutProvider>) {
        self.activate();
        LAYOUT_PROVIDER.with(|p| *p.borrow_mut() = Some(provider));
    }
}

fn run_module(text: &str, base: &str, ctx: &mut Context) {
    let source = Source::from_bytes(text).with_path(Path::new(base));
    let module = match Module::parse(source, None, ctx) {
        Ok(module) => module,
        Err(error) => {
            log_script_error(base, error);
            return;
        }
    };
    let promise = module.load_link_evaluate(ctx);
    ctx.run_jobs();
    if let boa_engine::builtins::promise::PromiseState::Rejected(error) = promise.state() {
        let message = error
            .to_string(ctx)
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_else(|_| "module failed".to_string());
        log_script_error(base, message);
    }
}

/// Loads `import`s through the page's own network path.
pub(crate) struct PageModuleLoader {
    loaded: RefCell<std::collections::HashMap<String, Module>>,
}

impl PageModuleLoader {
    pub(crate) fn new() -> Self {
        Self {
            loaded: RefCell::new(std::collections::HashMap::new()),
        }
    }
}

impl ModuleLoader for PageModuleLoader {
    fn load_imported_module(
        &self,
        referrer: Referrer,
        specifier: JsString,
        finish_load: Box<dyn FnOnce(JsResult<Module>, &mut Context)>,
        context: &mut Context,
    ) {
        let specifier = specifier.to_std_string_escaped();
        let base = referrer
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(location_href);
        let resolved = url::Url::parse(&base)
            .ok()
            .and_then(|b| b.join(&specifier).ok())
            .map(|u| u.to_string());
        let Some(resolved) = resolved else {
            let error = JsNativeError::typ().with_message(format!("bad import: {specifier}"));
            return finish_load(Err(error.into()), context);
        };
        if let Some(module) = self.loaded.borrow().get(&resolved).cloned() {
            return finish_load(Ok(module), context);
        }
        let text = start_script_fetch(&resolved).and_then(|rx| receive_script(&rx));
        let Some(text) = text else {
            let error = JsNativeError::typ().with_message(format!("failed to load {resolved}"));
            return finish_load(Err(error.into()), context);
        };
        let source = Source::from_bytes(text.as_bytes()).with_path(Path::new(&resolved));
        let module = Module::parse(source, None, context);
        if let Ok(module) = &module {
            self.loaded.borrow_mut().insert(resolved, module.clone());
        }
        finish_load(module, context);
    }
}

/// A script element put into the document by script runs, unless it already has.
/// https://html.spec.whatwg.org/multipage/scripting.html#prepare-the-script-element
pub(crate) fn run_inserted_scripts(node: &Rc<RefCell<Node>>, ctx: &mut Context) {
    let mut connected = false;
    let mut ancestor = Some(node.clone());
    while let Some(a) = ancestor {
        if matches!(a.borrow().kind, NodeKind::Document) {
            connected = true;
            break;
        }
        ancestor = a.borrow().parent().upgrade();
    }
    if !connected {
        return;
    }
    let mut scripts = Vec::new();
    if node.borrow().element_tag_name().as_deref() == Some("script") {
        scripts.push(node.clone());
    }
    collect_scripts(node, &mut scripts);
    for script in scripts {
        let first_time =
            STARTED_SCRIPTS.with(|started| started.borrow_mut().insert(node_key(&script)));
        if !first_time {
            continue;
        }
        let Some(kind) = script_kind(&script) else {
            continue;
        };
        match attribute_of(&script, "src").filter(|s| !s.trim().is_empty()) {
            Some(src) => {
                if let Some(rx) = start_script_fetch(&src) {
                    PENDING_SCRIPTS.with(|p| {
                        p.borrow_mut().push(PendingScript {
                            node: script.clone(),
                            kind_is_module: kind == ScriptKind::Module,
                            url: src.clone(),
                            rx,
                        })
                    });
                }
            }
            None => {
                let text = script_text(&script);
                if text.trim().is_empty() {
                    continue;
                }
                CURRENT_SCRIPT.with(|s| *s.borrow_mut() = Some(script.clone()));
                match kind {
                    ScriptKind::Classic => {
                        if let Err(error) = ctx.eval(Source::from_bytes(text.as_str())) {
                            log_script_error("inserted script", error);
                        }
                    }
                    ScriptKind::Module => run_module(&text, &location_href(), ctx),
                }
                CURRENT_SCRIPT.with(|s| *s.borrow_mut() = None);
            }
        }
    }
}

/// Run dynamically inserted scripts whose fetches have finished.
pub(crate) fn pump_inserted_scripts(ctx: &mut Context) -> usize {
    let ready: Vec<(PendingScript, Option<FetchResponse>)> = PENDING_SCRIPTS.with(|pending| {
        let mut pending = pending.borrow_mut();
        let mut ready = Vec::new();
        let mut still = Vec::new();
        for script in pending.drain(..) {
            match script.rx.try_recv() {
                Ok(response) => ready.push((script, Some(response))),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => ready.push((script, None)),
                Err(std::sync::mpsc::TryRecvError::Empty) => still.push(script),
            }
        }
        *pending = still;
        ready
    });
    let count = ready.len();
    for (script, response) in ready {
        let text = response.filter(|r| r.ok && r.error.is_none()).map(|r| r.body);
        match text {
            Some(text) => {
                CURRENT_SCRIPT.with(|s| *s.borrow_mut() = Some(script.node.clone()));
                if script.kind_is_module {
                    run_module(&text, &script.url, ctx);
                } else if let Err(error) = ctx.eval(Source::from_bytes(text.as_str())) {
                    log_script_error(&script.url, error);
                }
                CURRENT_SCRIPT.with(|s| *s.borrow_mut() = None);
                fire_simple_event(&script.node, "load", ctx);
            }
            None => {
                log_script_error("failed to load script", &script.url);
                fire_simple_event(&script.node, "error", ctx);
            }
        }
    }
    count
}
