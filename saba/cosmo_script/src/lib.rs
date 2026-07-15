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
use cosmo_engine::renderer::dom::api::{collect_text, get_element_by_id};
use cosmo_engine::renderer::dom::node::{Node, NodeKind};
use std::cell::RefCell;
use std::rc::Rc;

/// Host data attached to an `Element` JsObject: a handle to the live DOM
/// node, kept outside Boa's GC. The Rc/RefCell contain no GC-managed
/// values, so ignoring them for tracing is sound.
#[derive(Trace, Finalize, JsData)]
struct NodeHandle {
    #[unsafe_ignore_trace]
    node: Rc<RefCell<Node>>,
}

/// Wrap a DOM node as an `Element` JsObject exposing a live `textContent`
/// accessor (get reads the node's text, set replaces its children).
fn make_element(node: Rc<RefCell<Node>>, context: &mut Context) -> JsObject {
    let obj = JsObject::from_proto_and_data(None, NodeHandle { node });
    let realm = context.realm().clone();
    let getter = NativeFunction::from_fn_ptr(element_get_text_content).to_js_function(&realm);
    let setter = NativeFunction::from_fn_ptr(element_set_text_content).to_js_function(&realm);
    let desc = PropertyDescriptor::builder()
        .get(getter)
        .set(setter)
        .enumerable(true)
        .configurable(true)
        .build();
    obj.insert_property(js_string!("textContent"), desc);
    obj
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
    // textContent setter replaces all children with a single text node.
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

    /// Expose the given document root to script as `document`.
    pub fn set_document(&mut self, root: Rc<RefCell<Node>>) {
        SCRIPT_DOM.with(|d| *d.borrow_mut() = Some(root));
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
            .build();
        self.context
            .register_global_property(js_string!("document"), document, Attribute::all())
            .expect("register document");
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
}
