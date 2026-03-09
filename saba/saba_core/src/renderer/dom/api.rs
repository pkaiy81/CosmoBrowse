use crate::renderer::dom::node::Element;
use crate::renderer::dom::node::ElementKind;
use crate::renderer::dom::node::Node;
use crate::renderer::dom::node::NodeKind;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::cell::RefCell;

pub fn get_element_by_id(
    node: Option<Rc<RefCell<Node>>>,
    id_name: &String,
) -> Option<Rc<RefCell<Node>>> {
    match node {
        Some(n) => {
            if let NodeKind::Element(e) = n.borrow().kind() {
                for attr in &e.attributes() {
                    if attr.name() == "id" && attr.value() == *id_name {
                        return Some(n.clone());
                    }
                }
            }
            let result1 = get_element_by_id(n.borrow().first_child(), id_name);
            let result2 = get_element_by_id(n.borrow().next_sibling(), id_name);
            if result1.is_none() {
                return result2;
            }
            result1
        }
        None => None,
    }
}

pub fn get_target_element_node(
    node: Option<Rc<RefCell<Node>>>,
    element_kind: ElementKind,
) -> Option<Rc<RefCell<Node>>> {
    match node {
        Some(n) => {
            if n.borrow().kind()
                == NodeKind::Element(Element::new(&element_kind.to_string(), Vec::new()))
            {
                return Some(n.clone());
            }
            let result1 = get_target_element_node(n.borrow().first_child(), element_kind);
            let result2 = get_target_element_node(n.borrow().next_sibling(), element_kind);
            if result1.is_none() && result2.is_none() {
                return None;
            }
            if result1.is_none() {
                return result2;
            }
            result1
        }
        None => None,
    }
}

fn collect_text(node: Option<Rc<RefCell<Node>>>, output: &mut String) {
    let Some(node) = node else {
        return;
    };

    if let NodeKind::Text(text) = node.borrow().kind() {
        output.push_str(&text);
    }

    collect_text(node.borrow().first_child(), output);
    collect_text(node.borrow().next_sibling(), output);
}

fn collect_tag_texts(node: Option<Rc<RefCell<Node>>>, kind: ElementKind, output: &mut Vec<String>) {
    let Some(node) = node else {
        return;
    };

    if node.borrow().element_kind() == Some(kind) {
        let mut text = String::new();
        collect_text(node.borrow().first_child(), &mut text);
        if !text.is_empty() {
            output.push(text);
        }
    }

    collect_tag_texts(node.borrow().first_child(), kind, output);
    collect_tag_texts(node.borrow().next_sibling(), kind, output);
}

fn collect_stylesheet_links_internal(node: Option<Rc<RefCell<Node>>>, output: &mut Vec<String>) {
    let Some(node) = node else {
        return;
    };

    if let NodeKind::Element(element) = node.borrow().kind() {
        if element.kind() == ElementKind::Link {
            let rel = element.get_attribute("rel").unwrap_or_default();
            if rel.eq_ignore_ascii_case("stylesheet") {
                if let Some(href) = element.get_attribute("href") {
                    output.push(href);
                }
            }
        }
    }

    collect_stylesheet_links_internal(node.borrow().first_child(), output);
    collect_stylesheet_links_internal(node.borrow().next_sibling(), output);
}

pub fn get_style_content(root: Rc<RefCell<Node>>) -> String {
    let mut styles = Vec::new();
    collect_tag_texts(Some(root), ElementKind::Style, &mut styles);
    styles.join("\n")
}

pub fn get_js_content(root: Rc<RefCell<Node>>) -> String {
    let mut scripts = Vec::new();
    collect_tag_texts(Some(root), ElementKind::Script, &mut scripts);
    scripts.join("\n")
}

pub fn get_title_content(root: Rc<RefCell<Node>>) -> Option<String> {
    let mut titles = Vec::new();
    collect_tag_texts(Some(root), ElementKind::Title, &mut titles);
    titles
        .into_iter()
        .find(|title| !title.trim().is_empty())
        .map(|title| title.trim().to_string())
}

pub fn get_stylesheet_links(root: Rc<RefCell<Node>>) -> Vec<String> {
    let mut links = Vec::new();
    collect_stylesheet_links_internal(Some(root), &mut links);
    links
}
