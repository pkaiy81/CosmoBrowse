//! JavaScript engine integration (plan Phase 3). Wraps boa_engine so the
//! rest of the browser can evaluate scripts without depending on Boa's API
//! surface directly — DOM bindings and the event loop layer on top of this.

use boa_engine::{
    js_string, object::ObjectInitializer, property::Attribute, Context, JsValue, NativeFunction,
    Source,
};
use cosmo_engine::renderer::dom::api::{collect_text, get_element_by_id};
use cosmo_engine::renderer::dom::node::Node;
use std::cell::RefCell;
use std::rc::Rc;

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
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let id = args
        .first()
        .and_then(|v| v.as_string())
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_default();
    let text = SCRIPT_DOM.with(|d| {
        d.borrow().as_ref().and_then(|dom| {
            get_element_by_id(Some(dom.clone()), &id).map(|el| {
                let mut s = String::new();
                collect_text(el.borrow().first_child(), &mut s);
                s
            })
        })
    });
    Ok(match text {
        Some(t) => JsValue::from(js_string!(t.as_str())),
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
    fn get_element_by_id_reads_dom_text() {
        let html =
            "<html><body><div id=\"greeting\">Hello DOM</div><p id=\"x\">other</p></body></html>";
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();

        let mut host = ScriptHost::new();
        host.set_document(document);
        assert_eq!(
            host.eval_to_string("document.getElementById('greeting')")
                .unwrap(),
            "Hello DOM"
        );
        assert_eq!(
            host.eval_to_string("document.getElementById('missing')")
                .unwrap(),
            "null"
        );
        // Script can compute over DOM-derived values.
        assert_eq!(
            host.eval_to_string("document.getElementById('greeting').length")
                .unwrap(),
            "9"
        );
    }
}
