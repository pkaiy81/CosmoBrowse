//! Selector matching against DOM nodes, extracted verbatim from
//! layout_object.rs (plan 0.5). Matching walks DOM relationships only, so it
//! can later serve querySelector()-style APIs without a layout tree.

use crate::renderer::css::cssom::{Selector};
use crate::renderer::dom::node::{Node, NodeKind};
use std::rc::Rc;
use std::cell::RefCell;

thread_local! {
    /// The elements whose `:has()` is being tested, innermost last. `HasSubject`
    /// matches only the last, so a `:has()` nested in another still works.
    static HAS_SUBJECT: RefCell<Vec<Rc<RefCell<Node>>>> = const { RefCell::new(Vec::new()) };
}

/// `:has(...)`: test `node`'s descendants, plus its later siblings and theirs,
/// which is everything a relative selector can reach.
fn matches_has(node: &Rc<RefCell<Node>>, selector: &Selector) -> bool {
    if !matches!(node.borrow().kind(), NodeKind::Element(_)) {
        return false;
    }
    fn any_within(start: Option<Rc<RefCell<Node>>>, selector: &Selector) -> bool {
        let mut current = start;
        while let Some(n) = current {
            if dom_node_selected(&n, selector) {
                return true;
            }
            if any_within(n.borrow().first_child(), selector) {
                return true;
            }
            current = n.borrow().next_sibling();
        }
        false
    }
    HAS_SUBJECT.with(|subject| subject.borrow_mut().push(node.clone()));
    let found = any_within(node.borrow().first_child(), selector)
        || any_within(node.borrow().next_sibling(), selector);
    HAS_SUBJECT.with(|subject| subject.borrow_mut().pop());
    found
}

/// The pseudo-classes read off the element's own tag and attributes.
fn form_or_link_state(
    node: &Rc<RefCell<Node>>,
    kind: &crate::renderer::css::cssom::PseudoClassKind,
) -> Option<bool> {
    use crate::renderer::css::cssom::PseudoClassKind as K;
    let borrowed = node.borrow();
    let NodeKind::Element(ref element) = borrowed.kind() else {
        return Some(false);
    };
    let tag = element.tag_name();
    let has = |name: &str| element.get_attribute(name).is_some();
    let is_control = matches!(
        tag,
        "input" | "button" | "select" | "textarea" | "option" | "optgroup" | "fieldset"
    );
    let is_field = matches!(tag, "input" | "select" | "textarea");
    let text_entry = match tag {
        "textarea" => true,
        "input" => !matches!(
            element.get_attribute("type").unwrap_or_default().to_ascii_lowercase().as_str(),
            "checkbox" | "radio" | "range" | "color" | "file" | "submit" | "reset" | "button"
                | "image" | "hidden"
        ),
        _ => false,
    };
    Some(match kind {
        K::Link => matches!(tag, "a" | "area") && has("href"),
        K::Checked => {
            (tag == "input"
                && matches!(
                    element.get_attribute("type").unwrap_or_default().to_ascii_lowercase().as_str(),
                    "checkbox" | "radio"
                )
                && has("checked"))
                || (tag == "option" && has("selected"))
        }
        K::Disabled => is_control && has("disabled"),
        K::Enabled => is_control && !has("disabled"),
        K::Empty => {
            let mut child = borrowed.first_child();
            let mut empty = true;
            while let Some(c) = child {
                match c.borrow().kind() {
                    NodeKind::Text(ref t) if t.is_empty() => {}
                    _ => empty = false,
                }
                child = c.borrow().next_sibling();
            }
            empty
        }
        K::Required => is_field && has("required"),
        K::Optional => is_field && !has("required"),
        K::ReadWrite => text_entry && !has("readonly") && !has("disabled"),
        K::ReadOnly => !(text_entry && !has("readonly") && !has("disabled")),
        K::PlaceholderShown => {
            text_entry
                && has("placeholder")
                && element.get_attribute("value").unwrap_or_default().is_empty()
        }
        _ => return None,
    })
}

/// Selector matching against a DOM node. Combinators walk DOM relationships
/// (parent / preceding siblings); simple selectors test the element itself.
pub(crate) fn dom_node_selected(node: &Rc<RefCell<Node>>, selector: &Selector) -> bool {
    // Combinators and grouping first: they recurse into simple matching.
    match selector {
        Selector::List(alternatives) => {
            return alternatives.iter().any(|s| dom_node_selected(node, s));
        }
        Selector::Compound(parts) => {
            return parts.iter().all(|s| dom_node_selected(node, s));
        }
        Selector::Child(ancestor, this) => {
            return dom_node_selected(node, this)
                && node
                    .borrow()
                    .parent()
                    .upgrade()
                    .map(|p| dom_node_selected(&p, ancestor))
                    .unwrap_or(false);
        }
        Selector::Descendant(ancestor, this) => {
            if !dom_node_selected(node, this) {
                return false;
            }
            let mut current = node.borrow().parent().upgrade();
            while let Some(p) = current {
                if dom_node_selected(&p, ancestor) {
                    return true;
                }
                let next = p.borrow().parent().upgrade();
                current = next;
            }
            return false;
        }
        Selector::NextSibling(prev, this) | Selector::SubsequentSibling(prev, this) => {
            if !dom_node_selected(node, this) {
                return false;
            }
            let adjacent_only = matches!(selector, Selector::NextSibling(..));
            // Walk the parent's children up to this node, tracking preceding
            // ELEMENT siblings (text nodes are not siblings for + / ~).
            let parent = match node.borrow().parent().upgrade() {
                Some(p) => p,
                None => return false,
            };
            let mut matched_any = false;
            let mut last_was_match = false;
            let mut child = parent.borrow().first_child();
            while let Some(c) = child {
                if Rc::ptr_eq(&c, node) {
                    return if adjacent_only { last_was_match } else { matched_any };
                }
                if matches!(c.borrow().kind(), NodeKind::Element(_)) {
                    let m = dom_node_selected(&c, prev);
                    last_was_match = m;
                    matched_any |= m;
                }
                let next = c.borrow().next_sibling();
                child = next;
            }
            return false;
        }
        Selector::Never => return false,
        // A pseudo-element rule never matches the host element itself (its
        // declarations would otherwise leak); the layout tree builder applies
        // it to a synthesized generated box via pseudo_element_target.
        Selector::PseudoElement(..) => return false,
        Selector::Not(inner) => {
            return matches!(node.borrow().kind(), NodeKind::Element(_))
                && !dom_node_selected(node, inner);
        }
        Selector::Is(inner) | Selector::Where(inner) => {
            return matches!(node.borrow().kind(), NodeKind::Element(_))
                && dom_node_selected(node, inner);
        }
        Selector::HasSubject => {
            return HAS_SUBJECT.with(|subject| {
                subject
                    .borrow()
                    .last()
                    .is_some_and(|candidate| Rc::ptr_eq(candidate, node))
            });
        }
        Selector::Has(inner) => return matches_has(node, inner),
        Selector::PseudoClass(kind) => {
            if !matches!(node.borrow().kind(), NodeKind::Element(_)) {
                return false;
            }
            use crate::renderer::css::cssom::PseudoClassKind;
            // `:hover` consults the pointer state the renderer published for
            // this style pass (the hovered element and its ancestors).
            let key = Rc::as_ptr(node) as *const () as usize;
            if matches!(kind, PseudoClassKind::Hover) {
                return crate::renderer::style::values::is_hovered(key);
            }
            // Focus state, published by the renderer the same way hover is.
            if matches!(kind, PseudoClassKind::Focus) {
                return crate::renderer::style::values::is_focused(key);
            }
            if matches!(kind, PseudoClassKind::FocusWithin) {
                return crate::renderer::style::values::is_focus_within(key);
            }
            if let Some(result) = form_or_link_state(node, kind) {
                return result;
            }
            // `:root` is the <html> element (it has no element parent).
            if matches!(kind, PseudoClassKind::Root) {
                let parent_is_element = node
                    .borrow()
                    .parent()
                    .upgrade()
                    .map(|p| matches!(p.borrow().kind(), NodeKind::Element(_)))
                    .unwrap_or(false);
                return !parent_is_element;
            }
            // 1-based index of this element among its parent's ELEMENT
            // children (and, for the *-of-type family, among element children
            // with the SAME tag), plus the respective totals.
            let own_tag = match node.borrow().kind() {
                NodeKind::Element(ref e) => e.tag_name().to_string(),
                _ => return false,
            };
            let parent = match node.borrow().parent().upgrade() {
                Some(p) => p,
                None => return false,
            };
            let mut index = 0usize;
            let mut total = 0usize;
            let mut index_of_type = 0usize;
            let mut total_of_type = 0usize;
            let mut child = parent.borrow().first_child();
            while let Some(c) = child {
                if let NodeKind::Element(ref e) = c.borrow().kind() {
                    total += 1;
                    let same_type = e.tag_name() == own_tag;
                    if same_type {
                        total_of_type += 1;
                    }
                    if Rc::ptr_eq(&c, node) {
                        index = total;
                        index_of_type = total_of_type;
                    }
                }
                let next = c.borrow().next_sibling();
                child = next;
            }
            if index == 0 {
                return false;
            }
            // i matches An+B when i = A*n + B for some integer n ≥ 0.
            let nth_matches = |a: i64, b: i64, i: i64| -> bool {
                if a == 0 {
                    i == b
                } else {
                    let d = i - b;
                    d % a == 0 && d / a >= 0
                }
            };
            return match kind {
                PseudoClassKind::Hover
                | PseudoClassKind::Focus
                | PseudoClassKind::FocusWithin
                | PseudoClassKind::Root
                | PseudoClassKind::Link
                | PseudoClassKind::Checked
                | PseudoClassKind::Disabled
                | PseudoClassKind::Enabled
                | PseudoClassKind::Empty
                | PseudoClassKind::Required
                | PseudoClassKind::Optional
                | PseudoClassKind::ReadOnly
                | PseudoClassKind::ReadWrite
                | PseudoClassKind::PlaceholderShown => unreachable!("handled above"),
                PseudoClassKind::FirstChild => index == 1,
                PseudoClassKind::LastChild => index == total,
                PseudoClassKind::OnlyChild => total == 1,
                PseudoClassKind::NthChild(a, b) => nth_matches(*a, *b, index as i64),
                PseudoClassKind::NthLastChild(a, b) => {
                    nth_matches(*a, *b, (total - index + 1) as i64)
                }
                PseudoClassKind::FirstOfType => index_of_type == 1,
                PseudoClassKind::LastOfType => index_of_type == total_of_type,
                PseudoClassKind::OnlyOfType => total_of_type == 1,
                PseudoClassKind::NthOfType(a, b) => nth_matches(*a, *b, index_of_type as i64),
                PseudoClassKind::NthLastOfType(a, b) => {
                    nth_matches(*a, *b, (total_of_type - index_of_type + 1) as i64)
                }
            };
        }
        _ => {}
    }
    match node.borrow().kind() {
        NodeKind::Element(ref e) => match selector {
            Selector::TypeSelector(type_name) => e.tag_name() == *type_name,
            // An element may carry several space-separated class names;
            // the selector matches any one of them.
            // https://html.spec.whatwg.org/multipage/dom.html#classes
            Selector::ClassSelector(class_name) => e.attributes().iter().any(|attr| {
                attr.name() == "class"
                    && attr.value().split_ascii_whitespace().any(|c| c == class_name)
            }),
            Selector::IdSelector(id_name) => e
                .attributes()
                .iter()
                .any(|attr| attr.name() == "id" && attr.value() == *id_name),
            Selector::Attribute { name, op, value } => e
                .attributes()
                .iter()
                .filter(|attr| attr.name().eq_ignore_ascii_case(name))
                .any(|attr| {
                    let v = attr.value();
                    use crate::renderer::css::cssom::AttrOp;
                    match op {
                        AttrOp::Exists => true,
                        AttrOp::Equals => v == *value,
                        AttrOp::Includes => v.split_ascii_whitespace().any(|w| w == value),
                        AttrOp::DashMatch => {
                            v == *value || v.starts_with(&format!("{}-", value))
                        }
                        AttrOp::Prefix => !value.is_empty() && v.starts_with(value.as_str()),
                        AttrOp::Suffix => !value.is_empty() && v.ends_with(value.as_str()),
                        AttrOp::Substring => !value.is_empty() && v.contains(value.as_str()),
                    }
                }),
            Selector::Universal => true,
            Selector::UnknownSelector => false,
            // Handled above; unreachable here.
            _ => false,
        },
        _ => false,
    }
}
