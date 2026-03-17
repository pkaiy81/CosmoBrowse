use alloc::string::String;
use alloc::vec::Vec;
use cosmo_core_legacy::renderer::js::runtime::JsRuntime;

/// JS runtime integration boundary exposed by `cosmo_core`.
///
/// Spec alignment:
/// - HTML LS defines that scripting integrates with DOM via event dispatch and task queue processing.
///   https://html.spec.whatwg.org/multipage/webappapis.html#event-loop-processing-model
/// - DOM Standard defines event dispatch and listener invocation at EventTarget boundaries.
///   https://dom.spec.whatwg.org/#interface-eventtarget
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomRuntimeEvent {
    DomContentLoaded,
    Click,
    Input,
    Change,
}

impl DomRuntimeEvent {
    pub fn as_dom_name(&self) -> &'static str {
        match self {
            Self::DomContentLoaded => "DOMContentLoaded",
            Self::Click => "click",
            Self::Input => "input",
            Self::Change => "change",
        }
    }
}

/// Bridge trait for DOM-facing runtime engines.
///
/// This keeps the boundary explicit while allowing `cosmo_core_legacy` runtime internals
/// to evolve independently from app/runtime callers.
pub trait JsDomRuntimeBridge {
    fn execute_bootstrap(&mut self);

    fn dispatch_dom_event_by_id(&mut self, event: DomRuntimeEvent, target_id: &str);

    fn diagnostics(&self) -> Vec<String>;
}

impl JsDomRuntimeBridge for JsRuntime {
    fn execute_bootstrap(&mut self) {}

    fn dispatch_dom_event_by_id(&mut self, event: DomRuntimeEvent, target_id: &str) {
        match event {
            DomRuntimeEvent::DomContentLoaded => {}
            DomRuntimeEvent::Click => self.dispatch_click(target_id),
            DomRuntimeEvent::Input => self.dispatch_input(target_id),
            DomRuntimeEvent::Change => self.dispatch_change(target_id),
        }
    }

    fn diagnostics(&self) -> Vec<String> {
        self.unsupported_apis()
    }
}
