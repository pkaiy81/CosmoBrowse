use crate::renderer::dom::api::DomApiBinding;
use crate::renderer::dom::api::DomEventType;
use crate::renderer::dom::node::Node as DomNode;
use crate::renderer::js::ast::Node;
use crate::renderer::js::ast::Program;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::borrow::Borrow;
use core::cell::RefCell;
use core::fmt::Display;
use core::fmt::Formatter;
use core::ops::Add;
use core::ops::Sub;

type VariableMap = Vec<(String, Option<RuntimeValue>)>;

const MAX_EVENT_LOOP_ITERATIONS: usize = 10_000;
const MAX_MICROTASK_DRAIN_ITERATIONS: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
struct TimerTask {
    due_turn: u64,
    callback: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeValue {
    /// https://262.ecma-international.org/#sec-numeric-types
    Number(u64),
    StringLiteral(String),
    HtmlElement {
        object: Rc<RefCell<DomNode>>,
        property: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    id: String,
    params: Vec<Option<Rc<Node>>>,
    body: Option<Rc<Node>>,
}

impl Function {
    fn new(id: String, params: Vec<Option<Rc<Node>>>, body: Option<Rc<Node>>) -> Self {
        Self { id, params, body }
    }
}

/// https://262.ecma-international.org/#sec-environment-records
#[derive(Debug, Clone)]
pub struct Environment {
    variables: VariableMap,
    outer: Option<Rc<RefCell<Environment>>>,
}

impl Environment {
    fn new(outer: Option<Rc<RefCell<Environment>>>) -> Self {
        Self {
            variables: VariableMap::new(),
            outer,
        }
    }

    // p. 394
    pub fn get_variable(&self, name: String) -> Option<RuntimeValue> {
        for variable in &self.variables {
            if variable.0 == name {
                return variable.1.clone(); // 1
            }
        }
        if let Some(env) = &self.outer {
            return env.borrow_mut().get_variable(name); // 2
        } else {
            None
        }
    }

    fn add_variable(&mut self, name: String, value: Option<RuntimeValue>) {
        self.variables.push((name, value));
    }

    fn update_variable(&mut self, name: String, value: Option<RuntimeValue>) {
        for i in 0..self.variables.len() {
            // If the variable is found in the current environment,
            // remove the current name and value, and add the new name and value.
            if self.variables[i].0 == name {
                self.variables.remove(i);
                self.variables.push((name, value));
                return;
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct JsRuntime {
    dom_api: DomApiBinding,
    env: Rc<RefCell<Environment>>,
    functions: Vec<Function>,
    task_queue: Vec<String>,
    microtask_queue: Vec<String>,
    timer_queue: Vec<TimerTask>,
    unsupported_apis: Vec<String>,
    warning_logs: Vec<String>,
    event_loop_turn: u64,
    next_timer_id: u64,
    render_pipeline_invalidated: bool,
    dom_content_loaded_listeners: Vec<String>,
}

impl JsRuntime {
    pub fn new(dom_root: Rc<RefCell<DomNode>>) -> Self {
        Self {
            dom_api: DomApiBinding::new(dom_root),
            functions: Vec::new(),
            env: Rc::new(RefCell::new(Environment::new(None))),
            task_queue: Vec::new(),
            microtask_queue: Vec::new(),
            timer_queue: Vec::new(),
            unsupported_apis: Vec::new(),
            warning_logs: Vec::new(),
            event_loop_turn: 0,
            next_timer_id: 1,
            render_pipeline_invalidated: false,
            dom_content_loaded_listeners: Vec::new(),
        }
    }

    pub fn execute(&mut self, program: &Program) {
        // Spec: HTML event loop processes one task, then drains the microtask queue.
        // https://html.spec.whatwg.org/multipage/webappapis.html#event-loop-processing-model
        for node in program.body() {
            self.eval(&Some(node.clone()), self.env.clone());
        }
        // Spec: running script is a task; perform a microtask checkpoint after script execution.
        // https://html.spec.whatwg.org/multipage/webappapis.html#perform-a-microtask-checkpoint
        self.drain_microtasks();
        self.fire_dom_content_loaded();
        self.process_event_loop();
    }

    // Spec: DOM events are dispatched to an event target and invoke registered listeners.
    // https://dom.spec.whatwg.org/#concept-event-dispatch
    pub fn dispatch_click(&mut self, target_id: &str) {
        self.dispatch_event(target_id, DomEventType::Click);
    }

    pub fn dispatch_input(&mut self, target_id: &str) {
        self.dispatch_event(target_id, DomEventType::Input);
    }

    pub fn dispatch_change(&mut self, target_id: &str) {
        self.dispatch_event(target_id, DomEventType::Change);
    }

    fn dispatch_event(&mut self, target_id: &str, event_type: DomEventType) {
        if let Some(target) = self.dom_api.document_get_element_by_id(target_id) {
            let callbacks = self.dom_api.dispatch_event(target, event_type);
            for callback in callbacks {
                self.task_queue.push(callback);
            }
            self.process_event_loop();
        }
    }

    pub fn unsupported_apis(&self) -> Vec<String> {
        self.unsupported_apis.clone()
    }

    pub fn warning_logs(&self) -> Vec<String> {
        self.warning_logs.clone()
    }

    pub fn dom_updated(&self) -> bool {
        self.render_pipeline_invalidated
    }

    pub fn render_pipeline_invalidated(&self) -> bool {
        self.render_pipeline_invalidated
    }

    fn process_event_loop(&mut self) {
        let mut iterations = 0;
        loop {
            iterations += 1;
            if iterations > MAX_EVENT_LOOP_ITERATIONS {
                self.warn(
                    "Event loop iteration guard triggered; possible hang/starvation detected"
                        .to_string(),
                );
                break;
            }

            // Spec: timer task source feeds into task queues, and the loop picks one task at a time.
            // https://html.spec.whatwg.org/multipage/timers-and-user-prompts.html#timers
            self.promote_ready_timers_to_tasks();

            if !self.task_queue.is_empty() {
                let callback = self.task_queue.remove(0);
                self.invoke_named_function(&callback, self.env.clone());

                // Spec: after each task, drain the microtask queue before selecting the next task.
                // https://html.spec.whatwg.org/multipage/webappapis.html#event-loop-processing-model
                // Spec: Promise reactions are queued as ECMAScript jobs (microtasks).
                // https://262.ecma-international.org/#sec-jobs-and-job-queues
                self.drain_microtasks();
                self.event_loop_turn = self.event_loop_turn.saturating_add(1);
                continue;
            }

            if self.timer_queue.is_empty() {
                break;
            }

            // Spec-aligned simplification: if only future timer tasks remain, advance to next turn.
            self.event_loop_turn = self.event_loop_turn.saturating_add(1);
        }
    }

    fn promote_ready_timers_to_tasks(&mut self) {
        let mut i = 0;
        while i < self.timer_queue.len() {
            if self.timer_queue[i].due_turn <= self.event_loop_turn {
                let timer = self.timer_queue.remove(i);
                self.task_queue.push(timer.callback);
            } else {
                i += 1;
            }
        }
    }

    fn fire_dom_content_loaded(&mut self) {
        // Spec: DOMContentLoaded fires after parsing and deferred script execution completes.
        // https://html.spec.whatwg.org/multipage/parsing.html#the-end
        for callback in self.dom_content_loaded_listeners.clone() {
            self.task_queue.push(callback);
        }
    }

    fn drain_microtasks(&mut self) {
        let mut iterations = 0;
        while !self.microtask_queue.is_empty() {
            iterations += 1;
            if iterations > MAX_MICROTASK_DRAIN_ITERATIONS {
                self.warn(
                    "Microtask drain guard triggered; possible infinite Promise/microtask chain"
                        .to_string(),
                );
                self.microtask_queue.clear();
                break;
            }
            let microtask = self.microtask_queue.remove(0);
            self.invoke_named_function(&microtask, self.env.clone());
        }
    }

    fn warn(&mut self, message: String) {
        self.warning_logs.push(message);
    }

    fn invoke_named_function(&mut self, callback: &str, env: Rc<RefCell<Environment>>) {
        let function_body = self
            .functions
            .iter()
            .find(|func| func.id == callback)
            .and_then(|func| func.body.clone());
        if let Some(body) = function_body {
            let new_env = Rc::new(RefCell::new(Environment::new(Some(env))));
            self.eval(&Some(body), new_env);
            return;
        }
        self.report_unsupported_api(format!("callback '{}'", callback));
    }

    fn report_unsupported_api(&mut self, api: String) {
        self.unsupported_apis
            .push(format!("Unsupported browser API: {}", api));
    }

    /// Return a tuple of (bool, Option<RuntimeValue>)
    /// bool: Whether the browser API was called or not, true indicates that some API was called
    /// Option<RuntimeValue>: the result obtained by calling the browser API
    /// p.426
    fn call_browser_api(
        &mut self,
        func: &RuntimeValue,
        arguments: &[Option<Rc<Node>>],
        env: Rc<RefCell<Environment>>,
    ) -> (bool, Option<RuntimeValue>) {
        if func == &RuntimeValue::StringLiteral("document.getElementById".to_string()) {
            // 1
            let arg = match self.eval(&arguments[0], env.clone()) {
                // 2
                Some(a) => a,
                None => return (true, None),
            };
            let target = match self.dom_api.document_get_element_by_id(&arg.to_string()) {
                // 3
                Some(n) => n,
                None => return (true, None),
            };
            return (
                true,
                Some(RuntimeValue::HtmlElement {
                    // 4
                    object: target,
                    property: None,
                }),
            );
        }

        if func == &RuntimeValue::StringLiteral("document.addEventListener".to_string()) {
            if arguments.len() < 2 {
                self.report_unsupported_api("document.addEventListener".to_string());
                return (true, None);
            }
            let event_type = self.eval(&arguments[0], env.clone());
            let callback = self.eval(&arguments[1], env.clone());
            if event_type == Some(RuntimeValue::StringLiteral("DOMContentLoaded".to_string())) {
                if let Some(callback) = callback.map(|v| v.to_string()) {
                    self.dom_content_loaded_listeners.push(callback);
                    return (true, None);
                }
            }
            self.report_unsupported_api("document.addEventListener".to_string());
            return (true, None);
        }

        if func == &RuntimeValue::StringLiteral("setTimeout".to_string()) {
            if arguments.is_empty() {
                self.report_unsupported_api("setTimeout(callback, delay)".to_string());
                return (true, None);
            }
            if let Some(callback) = self.eval(&arguments[0], env.clone()).map(|v| v.to_string()) {
                // Spec: timers queue tasks in the event loop task queue.
                // https://html.spec.whatwg.org/multipage/timers-and-user-prompts.html#timers
                // Compliance note: this minimum runtime assigns timers to a dedicated timer queue,
                // then moves ready entries to the macrotask queue when the event-loop turn advances.
                let timer_id = self.next_timer_id;
                self.next_timer_id = self.next_timer_id.saturating_add(1);
                self.timer_queue.push(TimerTask {
                    due_turn: self.event_loop_turn.saturating_add(1),
                    callback,
                });
                return (true, Some(RuntimeValue::Number(timer_id)));
            }
            self.report_unsupported_api("setTimeout(callback, delay)".to_string());
            return (true, None);
        }

        if func == &RuntimeValue::StringLiteral("queueMicrotask".to_string())
            || func == &RuntimeValue::StringLiteral("Promise.then".to_string())
        {
            if arguments.is_empty() {
                self.report_unsupported_api("queueMicrotask(callback)/Promise.then".to_string());
                return (true, None);
            }
            if let Some(callback) = self.eval(&arguments[0], env.clone()).map(|v| v.to_string()) {
                // Spec: host hooks queue Promise reactions as ECMAScript jobs (microtasks).
                // https://262.ecma-international.org/#sec-jobs-and-job-queues
                self.microtask_queue.push(callback);
                return (true, None);
            }
            self.report_unsupported_api("queueMicrotask(callback)/Promise.then".to_string());
            return (true, None);
        }

        if let RuntimeValue::HtmlElement {
            object,
            property: Some(property),
        } = func
        {
            if property == "addEventListener" {
                if arguments.len() < 2 {
                    self.report_unsupported_api("Element.addEventListener".to_string());
                    return (true, None);
                }
                let event_type = self.eval(&arguments[0], env.clone());
                let callback = self.eval(&arguments[1], env.clone());
                if let Some(RuntimeValue::StringLiteral(event_name)) = event_type {
                    if let Some(event_type) = DomEventType::from_name(&event_name) {
                        if let Some(callback) = callback.map(|v| v.to_string()) {
                            self.dom_api.element_add_event_listener(
                                object.clone(),
                                event_type,
                                callback,
                            );
                            return (true, None);
                        }
                    }
                }
                self.report_unsupported_api("Element.addEventListener".to_string());
                return (true, None);
            }
        }

        (false, None)
    }

    // p.372
    // p.395 X
    // p.419 Y
    // Spec: function calls and environment chaining follow ECMAScript execution contexts.
    // https://262.ecma-international.org/#sec-execution-contexts
    fn eval(
        &mut self,
        node: &Option<Rc<Node>>,
        env: Rc<RefCell<Environment>>, // X1
    ) -> Option<RuntimeValue> {
        let node = match node {
            Some(n) => n,
            None => return None,
        };

        match node.borrow() {
            Node::ExpressionStatement(expr) => return self.eval(&expr, env.clone()), // X1
            Node::AdditiveExpression {
                operator,
                left,
                right,
            } => {
                // X3
                // 2
                let left_value = match self.eval(left, env.clone()) {
                    // 3
                    Some(value) => value,
                    None => return None,
                };
                let right_value = match self.eval(right, env.clone()) {
                    // 4
                    Some(value) => value,
                    None => return None,
                };

                if operator == &'+' {
                    Some(left_value + right_value) // 5
                } else if operator == &'-' {
                    Some(left_value - right_value) // 6
                } else {
                    None
                }
            }
            Node::AssignmentExpression {
                operator,
                left,
                right,
            } => {
                if operator != &'=' {
                    return None;
                }
                // Reassign variable.
                if let Some(node) = left {
                    if let Node::Identifier(id) = node.borrow() {
                        let new_value = self.eval(right, env.clone());
                        env.borrow_mut().update_variable(id.to_string(), new_value); // X4
                        return None;
                    }
                }

                // If the left-hand value represents a HtmlElement of the DOM tree, update the DOM tree
                if let Some(RuntimeValue::HtmlElement { object, property }) =
                    self.eval(left, env.clone())
                {
                    let right_value = match self.eval(right, env.clone()) {
                        Some(value) => value,
                        None => return None,
                    };

                    if let Some(p) = property {
                        // Change the text of the node like target.textContent = "foobar";
                        if p == "textContent" {
                            self.dom_api
                                .element_set_text_content(object.clone(), &right_value.to_string());
                            // Spec: DOM mutation invalidates render output and requires update-the-rendering integration.
                            // https://html.spec.whatwg.org/multipage/webappapis.html#update-the-rendering
                            self.render_pipeline_invalidated = true;
                        } else if p == "onclick" {
                            self.dom_api.element_add_event_listener(
                                object.clone(),
                                DomEventType::Click,
                                right_value.to_string(),
                            );
                        } else if p == "oninput" {
                            self.dom_api.element_add_event_listener(
                                object.clone(),
                                DomEventType::Input,
                                right_value.to_string(),
                            );
                        } else if p == "onchange" {
                            self.dom_api.element_add_event_listener(
                                object.clone(),
                                DomEventType::Change,
                                right_value.to_string(),
                            );
                        }
                    }
                }
                None
            }
            Node::MemberExpression { object, property } => {
                let object_value = match self.eval(object, env.clone()) {
                    Some(value) => value,
                    None => return None,
                };
                let property_value = match self.eval(property, env.clone()) {
                    Some(value) => value,
                    // Return `object_value` here because the property does not exist
                    None => return Some(object_value),
                };

                // If the object is a DOM node, update the `property` of the HtmlElement
                // https://dom.spec.whatwg.org/#dom-node-textcontent
                if let RuntimeValue::HtmlElement { object, property } = object_value {
                    assert!(property.is_none());
                    // Set the `property_value` string to the `property` of the HtmlElement
                    return Some(RuntimeValue::HtmlElement {
                        object,
                        property: Some(property_value.to_string()),
                    });
                }

                // document.getElementById is treated as a single string, "document.getElementById".
                // A call to this method will result in a call to the function named "document.getElementById"
                return Some(
                    object_value + RuntimeValue::StringLiteral(".".to_string()) + property_value,
                );
            }
            Node::NumericLiteral(value) => Some(RuntimeValue::Number(*value)), // 7
            Node::VariableDeclaration { declarations } => {
                for declaration in declarations {
                    self.eval(&declaration, env.clone()); // X5
                }
                None
            }
            Node::VariableDeclarator { id, init } => {
                if let Some(node) = id {
                    if let Node::Identifier(id) = node.borrow() {
                        let init = self.eval(&init, env.clone());
                        env.borrow_mut().add_variable(id.to_string(), init); // X6
                    }
                }
                None
            }
            Node::Identifier(name) => {
                match env.borrow_mut().get_variable(name.to_string()) {
                    // X7
                    Some(v) => Some(v),
                    // When a variable name is used for the first time, it is treated as a String, since the value has not yet been stored.
                    // For example, in a code such as var a = 42;, a is treated as a StringLiteral.
                    None => Some(RuntimeValue::StringLiteral(name.to_string())), // X8
                }
            }
            Node::StringLiteral(value) => Some(RuntimeValue::StringLiteral(value.to_string())), // X9
            Node::BlockStatement { body } => {
                // Y1
                let mut result: Option<RuntimeValue> = None;
                for stmt in body {
                    result = self.eval(&stmt, env.clone());
                }
                result
            }
            Node::ReturnStatement { argument } => {
                // Y2
                return self.eval(&argument, env.clone());
            }
            Node::FunctionDeclaration { id, params, body } => {
                // Y3
                if let Some(RuntimeValue::StringLiteral(id)) = self.eval(&id, env.clone()) {
                    let cloned_body = match body {
                        Some(b) => Some(b.clone()),
                        None => None,
                    };
                    self.functions
                        .push(Function::new(id, params.to_vec(), cloned_body)); // Y4
                };
                None
            }
            Node::CallExpression { callee, arguments } => {
                // Y5
                // Create a new scope
                let new_env = Rc::new(RefCell::new(Environment::new(Some(env)))); // Y6

                let callee_value = match self.eval(callee, new_env.clone()) {
                    // Y7
                    Some(value) => value,
                    None => return None,
                };

                // Call the browser API
                // p.430
                let api_result = self.call_browser_api(&callee_value, arguments, new_env.clone());
                if api_result.0 {
                    // If the browser API was called, not executing the user defined function
                    return api_result.1;
                }

                if let RuntimeValue::StringLiteral(callee) = &callee_value {
                    if callee.contains('.') {
                        self.report_unsupported_api(callee.clone());
                        return None;
                    }
                }

                // Find the defined function.
                let function = {
                    // Y8
                    let mut f: Option<Function> = None;

                    for func in &self.functions {
                        if callee_value == RuntimeValue::StringLiteral(func.id.to_string()) {
                            f = Some(func.clone());
                        }
                    }

                    match f {
                        Some(f) => f,
                        None => {
                            self.report_unsupported_api(callee_value.to_string());
                            return None;
                        } // Y9
                    }
                };

                // Assign arguments passed at function call as local variables of the newly created scope
                assert!(arguments.len() == function.params.len());
                for (i, item) in arguments.iter().enumerate() {
                    if let Some(RuntimeValue::StringLiteral(name)) =
                        self.eval(&function.params[i], new_env.clone())
                    {
                        new_env
                            .borrow_mut()
                            .add_variable(name, self.eval(item, new_env.clone()));
                    } // Y10
                }

                // Calling a function with a new scope
                self.eval(&function.body.clone(), new_env.clone()) // Y11
            }
        }
    }
}

impl Add<RuntimeValue> for RuntimeValue {
    type Output = RuntimeValue;

    fn add(self, rhs: RuntimeValue) -> RuntimeValue {
        if let (RuntimeValue::Number(left_num), RuntimeValue::Number(right_num)) = (&self, &rhs) {
            return RuntimeValue::Number(left_num + right_num);
        }

        RuntimeValue::StringLiteral(self.to_string() + &rhs.to_string())
    }
}

impl Sub<RuntimeValue> for RuntimeValue {
    type Output = RuntimeValue;

    fn sub(self, rhs: RuntimeValue) -> RuntimeValue {
        if let (RuntimeValue::Number(left_num), RuntimeValue::Number(right_num)) = (&self, &rhs) {
            return RuntimeValue::Number(left_num - right_num);
        }

        // NaN: Not a Number
        RuntimeValue::Number(u64::MIN)
    }
}

impl Display for RuntimeValue {
    fn fmt(&self, f: &mut Formatter) -> core::fmt::Result {
        let s = match self {
            RuntimeValue::Number(value) => format!("{}", value),
            RuntimeValue::StringLiteral(value) => value.to_string(),
            RuntimeValue::HtmlElement {
                object,
                property: _,
            } => {
                format!("HtmlElement: {:#?}", object)
            }
        };
        write!(f, "{}", s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::dom::api::get_element_by_id;
    use crate::renderer::dom::node::NodeKind as DomNodeKind;
    use crate::renderer::html::parser::HtmlParser;
    use crate::renderer::html::token::HtmlTokenizer;
    use crate::renderer::js::ast::JsParser;
    use crate::renderer::js::token::JsLexer;

    #[test]
    fn test_num() {
        let dom = Rc::new(RefCell::new(DomNode::new(DomNodeKind::Document)));
        let input = "42".to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let ast = parser.parse_ast();
        let mut runtime = JsRuntime::new(dom);
        let expected = [Some(RuntimeValue::Number(42))];
        let mut i = 0;

        for node in ast.body() {
            let result = runtime.eval(&Some(node.clone()), runtime.env.clone());
            assert_eq!(expected[i], result);
            i += 1;
        }
    }

    #[test]
    fn test_add_nums() {
        let dom = Rc::new(RefCell::new(DomNode::new(DomNodeKind::Document)));
        let input = "1 + 2".to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let ast = parser.parse_ast();
        let mut runtime = JsRuntime::new(dom);
        let expected = [Some(RuntimeValue::Number(3))];
        let mut i = 0;

        for node in ast.body() {
            let result = runtime.eval(&Some(node.clone()), runtime.env.clone());
            assert_eq!(expected[i], result);
            i += 1;
        }
    }

    #[test]
    fn test_sub_nums() {
        let dom = Rc::new(RefCell::new(DomNode::new(DomNodeKind::Document)));
        let input = "2 - 1".to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let ast = parser.parse_ast();
        let mut runtime = JsRuntime::new(dom);
        let expected = [Some(RuntimeValue::Number(1))];
        let mut i = 0;

        for node in ast.body() {
            let result = runtime.eval(&Some(node.clone()), runtime.env.clone());
            assert_eq!(expected[i], result);
            i += 1;
        }
    }

    #[test]
    fn test_assign_variable() {
        let dom = Rc::new(RefCell::new(DomNode::new(DomNodeKind::Document)));
        let input = "var foo=42;".to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let ast = parser.parse_ast();
        let mut runtime = JsRuntime::new(dom);
        let expected = [None];
        let mut i = 0;

        for node in ast.body() {
            let result = runtime.eval(&Some(node.clone()), runtime.env.clone());
            assert_eq!(expected[i], result);
            i += 1;
        }
    }

    #[test]
    fn test_add_variable_and_num() {
        let dom = Rc::new(RefCell::new(DomNode::new(DomNodeKind::Document)));
        let input = "var foo=42; foo+1".to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let ast = parser.parse_ast();
        let mut runtime = JsRuntime::new(dom);
        let expected = [None, Some(RuntimeValue::Number(43))];
        let mut i = 0;

        for node in ast.body() {
            let result = runtime.eval(&Some(node.clone()), runtime.env.clone());
            assert_eq!(expected[i], result);
            i += 1;
        }
    }

    #[test]
    fn test_reassign_variable() {
        let dom = Rc::new(RefCell::new(DomNode::new(DomNodeKind::Document)));
        let input = "var foo=42; foo=1; foo".to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let ast = parser.parse_ast();
        let mut runtime = JsRuntime::new(dom);
        let expected = [None, None, Some(RuntimeValue::Number(1))];
        let mut i = 0;

        for node in ast.body() {
            let result = runtime.eval(&Some(node.clone()), runtime.env.clone());
            assert_eq!(expected[i], result);
            i += 1;
        }
    }

    #[test]
    fn test_add_function_and_num() {
        let dom = Rc::new(RefCell::new(DomNode::new(DomNodeKind::Document)));
        let input = "function foo() { return 42; } foo()+1".to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let ast = parser.parse_ast();
        let mut runtime = JsRuntime::new(dom);
        let expected = [None, Some(RuntimeValue::Number(43))];
        let mut i = 0;

        for node in ast.body() {
            let result = runtime.eval(&Some(node.clone()), runtime.env.clone());
            assert_eq!(expected[i], result);
            i += 1;
        }
    }

    #[test]
    fn test_define_function_with_args() {
        let dom = Rc::new(RefCell::new(DomNode::new(DomNodeKind::Document)));
        let input = "function foo(a, b) { return a + b; } foo(1, 2) + 3;".to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let ast = parser.parse_ast();
        let mut runtime = JsRuntime::new(dom);
        let expected = [None, Some(RuntimeValue::Number(6))];
        let mut i = 0;

        for node in ast.body() {
            let result = runtime.eval(&Some(node.clone()), runtime.env.clone());
            assert_eq!(expected[i], result);
            i += 1;
        }
    }

    #[test]
    fn test_event_loop_task_then_microtask_order() {
        let html = r#"<html><body><p id="target">init</p></body></html>"#.to_string();
        let window = HtmlParser::new(HtmlTokenizer::new(html)).construct_tree();
        let dom = window.as_ref().borrow().document();
        let input = r#"function task(){ document.getElementById("target").textContent="T"; queueMicrotask(micro); } function micro(){ document.getElementById("target").textContent="TM"; } setTimeout(task, 0);"#.to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let ast = parser.parse_ast();
        let mut runtime = JsRuntime::new(dom.clone());

        runtime.execute(&ast);

        let target = get_element_by_id(Some(dom), &"target".to_string()).expect("target");
        assert_eq!(
            target
                .as_ref()
                .borrow()
                .first_child()
                .expect("text")
                .as_ref()
                .borrow()
                .kind(),
            DomNodeKind::Text("TM".to_string())
        );
    }

    #[test]
    fn test_promise_then_queues_microtask() {
        let html = r#"<html><body><p id="target">init</p></body></html>"#.to_string();
        let window = HtmlParser::new(HtmlTokenizer::new(html)).construct_tree();
        let dom = window.as_ref().borrow().document();
        let input = r#"function reaction(){ document.getElementById("target").textContent="P"; } Promise.then(reaction);"#.to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let ast = parser.parse_ast();
        let mut runtime = JsRuntime::new(dom.clone());

        runtime.execute(&ast);

        let target = get_element_by_id(Some(dom), &"target".to_string()).expect("target");
        assert_eq!(
            target
                .as_ref()
                .borrow()
                .first_child()
                .expect("text")
                .as_ref()
                .borrow()
                .kind(),
            DomNodeKind::Text("P".to_string())
        );
    }

    #[test]
    fn test_microtask_runs_before_timer_task() {
        let html = r#"<html><body><p id="target">init</p></body></html>"#.to_string();
        let window = HtmlParser::new(HtmlTokenizer::new(html)).construct_tree();
        let dom = window.as_ref().borrow().document();
        let input = r#"function timer(){ document.getElementById("target").textContent=document.getElementById("target").textContent+"T"; } function reaction(){ document.getElementById("target").textContent=document.getElementById("target").textContent+"P"; } setTimeout(timer, 0); Promise.then(reaction);"#.to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let ast = parser.parse_ast();
        let mut runtime = JsRuntime::new(dom.clone());

        runtime.execute(&ast);

        let target = get_element_by_id(Some(dom), &"target".to_string()).expect("target");
        assert_eq!(
            target
                .as_ref()
                .borrow()
                .first_child()
                .expect("text")
                .as_ref()
                .borrow()
                .kind(),
            DomNodeKind::Text("initPT".to_string())
        );
    }

    #[test]
    fn test_microtask_guard_reports_warning() {
        let html = r#"<html><body><p id="target">init</p></body></html>"#.to_string();
        let window = HtmlParser::new(HtmlTokenizer::new(html)).construct_tree();
        let dom = window.as_ref().borrow().document();
        let input = r#"function loop(){ queueMicrotask(loop); } Promise.then(loop);"#.to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let ast = parser.parse_ast();
        let mut runtime = JsRuntime::new(dom);

        runtime.execute(&ast);

        assert!(runtime
            .warning_logs()
            .iter()
            .any(|log| log.contains("Microtask drain guard triggered")));
    }

    #[test]
    fn test_dom_content_loaded_handler_runs_after_bootstrap() {
        let html = r#"<html><body><p id="target">init</p></body></html>"#.to_string();
        let window = HtmlParser::new(HtmlTokenizer::new(html)).construct_tree();
        let dom = window.as_ref().borrow().document();
        let input = r#"function ready(){ document.getElementById("target").textContent="ready"; } document.addEventListener("DOMContentLoaded", ready);"#.to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let ast = parser.parse_ast();
        let mut runtime = JsRuntime::new(dom.clone());

        runtime.execute(&ast);

        let target = get_element_by_id(Some(dom), &"target".to_string()).expect("target");
        assert_eq!(
            target
                .as_ref()
                .borrow()
                .first_child()
                .expect("text")
                .as_ref()
                .borrow()
                .kind(),
            DomNodeKind::Text("ready".to_string())
        );
    }

    #[test]
    fn test_input_and_change_dispatch() {
        let html =
            r#"<html><body><input id="field" value="a" /><p id="target">init</p></body></html>"#
                .to_string();
        let window = HtmlParser::new(HtmlTokenizer::new(html)).construct_tree();
        let dom = window.as_ref().borrow().document();
        let input = r#"var field=document.getElementById("field"); function oninputhandler(){ document.getElementById("target").textContent="input"; } function onchangehandler(){ document.getElementById("target").textContent="change"; } field.addEventListener("input", oninputhandler); field.onchange = onchangehandler;"#.to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let ast = parser.parse_ast();
        let mut runtime = JsRuntime::new(dom.clone());

        runtime.execute(&ast);
        runtime.dispatch_input("field");
        runtime.dispatch_change("field");

        let target = get_element_by_id(Some(dom), &"target".to_string()).expect("target");
        assert_eq!(
            target
                .as_ref()
                .borrow()
                .first_child()
                .expect("text")
                .as_ref()
                .borrow()
                .kind(),
            DomNodeKind::Text("change".to_string())
        );
    }

    #[test]
    fn test_local_variable() {
        let dom = Rc::new(RefCell::new(DomNode::new(DomNodeKind::Document)));
        let input = "var a=42; function foo() { var a=1; return a; } foo()+a".to_string();
        let lexer = JsLexer::new(input);
        let mut parser = JsParser::new(lexer);
        let ast = parser.parse_ast();
        let mut runtime = JsRuntime::new(dom);
        let expected = [None, None, Some(RuntimeValue::Number(43))];
        let mut i = 0;

        for node in ast.body() {
            let result = runtime.eval(&Some(node.clone()), runtime.env.clone());
            assert_eq!(expected[i], result);
            i += 1;
        }
    }
}
