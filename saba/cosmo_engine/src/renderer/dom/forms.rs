//! Reading form controls' state, for submission from either the browser or script.
//! https://html.spec.whatwg.org/multipage/form-control-infrastructure.html

use crate::renderer::dom::node::{Node, NodeKind};
use std::cell::RefCell;
use std::rc::Rc;
use std::string::String;
use std::string::ToString;
use std::vec;
use std::vec::Vec;

type NodeRef = Rc<RefCell<Node>>;

pub fn tag(node: &NodeRef) -> String {
    node.borrow().element_tag_name().unwrap_or_default()
}

pub fn attribute(node: &NodeRef, name: &str) -> Option<String> {
    match node.borrow().kind {
        NodeKind::Element(ref e) => e.get_attribute(name),
        _ => None,
    }
}

pub fn input_type(node: &NodeRef) -> String {
    attribute(node, "type").unwrap_or_default().to_ascii_lowercase()
}

/// Every element in `root`'s subtree, in document order.
pub fn descendants(root: &NodeRef, out: &mut Vec<NodeRef>) {
    let mut child = root.borrow().first_child();
    while let Some(c) = child {
        if matches!(c.borrow().kind, NodeKind::Element(_)) {
            out.push(c.clone());
            descendants(&c, out);
        }
        let next = c.borrow().next_sibling();
        child = next;
    }
}

pub fn document_root(node: &NodeRef) -> NodeRef {
    let mut current = node.clone();
    loop {
        let parent = current.borrow().parent().upgrade();
        match parent {
            Some(p) => current = p,
            None => return current,
        }
    }
}

pub fn ancestors_and_self(node: &NodeRef) -> Vec<NodeRef> {
    let mut chain = vec![node.clone()];
    let mut current = node.borrow().parent().upgrade();
    while let Some(n) = current {
        chain.push(n.clone());
        current = n.borrow().parent().upgrade();
    }
    chain
}

pub fn is_disabled(node: &NodeRef) -> bool {
    if attribute(node, "disabled").is_some() {
        return true;
    }
    ancestors_and_self(node)
        .iter()
        .skip(1)
        .any(|n| tag(n) == "fieldset" && attribute(n, "disabled").is_some())
}

/// The selected `<option>`s of a `<select>`: those marked, or the first of them.
fn selected_options(select: &NodeRef) -> Vec<NodeRef> {
    let mut all = Vec::new();
    descendants(select, &mut all);
    let options: Vec<NodeRef> = all.into_iter().filter(|n| tag(n) == "option").collect();
    let marked: Vec<NodeRef> = options
        .iter()
        .filter(|o| attribute(o, "selected").is_some())
        .cloned()
        .collect();
    if !marked.is_empty() {
        if attribute(select, "multiple").is_some() {
            return marked;
        }
        return marked.into_iter().last().into_iter().collect();
    }
    if attribute(select, "multiple").is_some() {
        return Vec::new();
    }
    options.into_iter().take(1).collect()
}

/// An option's value: its `value` attribute, or else its text.
fn option_value(option: &NodeRef) -> String {
    attribute(option, "value").unwrap_or_else(|| {
        let mut text = String::new();
        crate::renderer::dom::api::collect_text(option.borrow().first_child(), &mut text);
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    })
}

/// The (name, value) pairs a submission sends.
/// https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#constructing-the-form-data-set
pub fn form_data(form: &NodeRef, submitter: Option<&NodeRef>) -> Vec<(String, String)> {
    let mut controls = Vec::new();
    descendants(form, &mut controls);
    // Controls outside the form can join with a `form` attribute.
    if let Some(id) = attribute(form, "id") {
        let mut all = Vec::new();
        descendants(&document_root(form), &mut all);
        for node in all {
            if attribute(&node, "form").as_deref() == Some(id.as_str())
                && !controls.iter().any(|c| Rc::ptr_eq(c, &node))
            {
                controls.push(node);
            }
        }
    }
    let mut entries = Vec::new();
    for control in controls {
        if is_disabled(&control) {
            continue;
        }
        let Some(name) = attribute(&control, "name").filter(|n| !n.is_empty()) else {
            continue;
        };
        match tag(&control).as_str() {
            "input" => match input_type(&control).as_str() {
                "checkbox" | "radio" => {
                    if attribute(&control, "checked").is_some() {
                        let value = attribute(&control, "value").unwrap_or_else(|| "on".to_string());
                        entries.push((name, value));
                    }
                }
                "submit" | "image" | "button" | "reset" => {
                    if submitter.is_some_and(|s| Rc::ptr_eq(s, &control)) {
                        entries.push((name, attribute(&control, "value").unwrap_or_default()));
                    }
                }
                "file" => {}
                _ => entries.push((name, attribute(&control, "value").unwrap_or_default())),
            },
            "button" => {
                if submitter.is_some_and(|s| Rc::ptr_eq(s, &control)) {
                    entries.push((name, attribute(&control, "value").unwrap_or_default()));
                }
            }
            "select" => {
                for option in selected_options(&control) {
                    entries.push((name.clone(), option_value(&option)));
                }
            }
            "textarea" => {
                let mut text = String::new();
                crate::renderer::dom::api::collect_text(
                    control.borrow().first_child(),
                    &mut text,
                );
                entries.push((name, text));
            }
            _ => {}
        }
    }
    entries
}

