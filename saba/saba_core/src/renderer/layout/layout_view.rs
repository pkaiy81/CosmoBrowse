use crate::display_item::DisplayItem;
use crate::renderer::css::cssom::StyleSheet;
use crate::renderer::dom::api::get_target_element_node;
use crate::renderer::dom::node::ElementKind;
use crate::renderer::dom::node::Node;
use crate::renderer::layout::layout_object::create_layout_object;
use crate::renderer::layout::layout_object::LayoutObject;
use crate::renderer::layout::layout_object::LayoutObjectKind;
use crate::renderer::layout::layout_object::LayoutPoint;
use crate::renderer::layout::layout_object::LayoutSize;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

fn build_layout_tree(
    node: &Option<Rc<RefCell<Node>>>,
    parent_obj: &Option<Rc<RefCell<LayoutObject>>>,
    cssom: &StyleSheet,
) -> Option<Rc<RefCell<LayoutObject>>> {
    let mut target_node = node.clone();
    let mut layout_object = create_layout_object(node, parent_obj, cssom);

    while layout_object.is_none() {
        if let Some(n) = target_node {
            target_node = n.borrow().next_sibling().clone();
            layout_object = create_layout_object(&target_node, parent_obj, cssom);
        } else {
            return layout_object;
        }
    }

    if let Some(n) = target_node {
        let original_first_child = n.borrow().first_child();
        let original_next_sibling = n.borrow().next_sibling();
        let mut first_child = build_layout_tree(&original_first_child, &layout_object, cssom);
        let mut next_sibling = build_layout_tree(&original_next_sibling, &None, cssom);

        if first_child.is_none() && original_first_child.is_some() {
            let mut original_dom_node = original_first_child
                .expect("first child should exist")
                .borrow()
                .next_sibling();

            loop {
                first_child = build_layout_tree(&original_dom_node, &layout_object, cssom);

                if first_child.is_none() && original_dom_node.is_some() {
                    original_dom_node = original_dom_node
                        .expect("next sibling should exist")
                        .borrow()
                        .next_sibling();
                    continue;
                }

                break;
            }
        }

        if next_sibling.is_none() && n.borrow().next_sibling().is_some() {
            let mut original_dom_node = original_next_sibling
                .expect("next sibling should exist")
                .borrow()
                .next_sibling();

            loop {
                next_sibling = build_layout_tree(&original_dom_node, &None, cssom);

                if next_sibling.is_none() && original_dom_node.is_some() {
                    original_dom_node = original_dom_node
                        .expect("next sibling should exist")
                        .borrow()
                        .next_sibling();
                    continue;
                }

                break;
            }
        }

        let obj = layout_object
            .as_ref()
            .expect("render object should exist here");
        obj.borrow_mut().set_first_child(first_child);
        obj.borrow_mut().set_next_sibling(next_sibling);
    }

    layout_object
}

#[derive(Debug, Clone)]
pub struct LayoutView {
    root: Option<Rc<RefCell<LayoutObject>>>,
    viewport_width: i64,
}

impl LayoutView {
    pub fn new(root: Rc<RefCell<Node>>, cssom: &StyleSheet, viewport_width: i64) -> Self {
        let body_root = get_target_element_node(Some(root), ElementKind::Body);

        let mut tree = Self {
            root: build_layout_tree(&body_root, &None, cssom),
            viewport_width: viewport_width.max(1),
        };

        tree.update_layout();
        tree
    }

    fn calculate_node_size(node: &Option<Rc<RefCell<LayoutObject>>>, parent_size: LayoutSize) {
        if let Some(n) = node {
            if n.borrow().kind() == LayoutObjectKind::Block {
                n.borrow_mut().compute_size(parent_size);
            }

            let child_parent_size = if n.borrow().kind() == LayoutObjectKind::Block {
                n.borrow().content_size()
            } else {
                parent_size
            };
            let first_child = n.borrow().first_child();
            Self::calculate_node_size(&first_child, child_parent_size);

            let next_sibling = n.borrow().next_sibling();
            Self::calculate_node_size(&next_sibling, parent_size);

            n.borrow_mut().compute_size(parent_size);
        }
    }

    fn calculate_node_position(
        node: &Option<Rc<RefCell<LayoutObject>>>,
        parent_point: LayoutPoint,
        parent_size: LayoutSize,
        previous_sibling_kind: LayoutObjectKind,
        previous_sibling_point: Option<LayoutPoint>,
        previous_sibling_size: Option<LayoutSize>,
    ) {
        if let Some(n) = node {
            n.borrow_mut().compute_position(
                parent_point,
                parent_size,
                previous_sibling_kind,
                previous_sibling_point,
                previous_sibling_size,
            );

            let first_child = n.borrow().first_child();
            Self::calculate_node_position(
                &first_child,
                n.borrow().content_origin(),
                n.borrow().content_size(),
                LayoutObjectKind::Block,
                None,
                None,
            );

            let next_sibling = n.borrow().next_sibling();
            Self::calculate_node_position(
                &next_sibling,
                parent_point,
                parent_size,
                n.borrow().kind(),
                Some(n.borrow().point()),
                Some(n.borrow().size()),
            );
        }
    }

    fn update_layout(&mut self) {
        let viewport_size = LayoutSize::new(self.viewport_width, 0);
        Self::calculate_node_size(&self.root, viewport_size);
        Self::calculate_node_position(
            &self.root,
            LayoutPoint::new(0, 0),
            viewport_size,
            LayoutObjectKind::Block,
            None,
            None,
        );
    }

    fn paint_node(node: &Option<Rc<RefCell<LayoutObject>>>, display_items: &mut Vec<DisplayItem>) {
        if let Some(n) = node {
            display_items.extend(n.borrow_mut().paint());
            let first_child = n.borrow().first_child();
            Self::paint_node(&first_child, display_items);
            let next_sibling = n.borrow().next_sibling();
            Self::paint_node(&next_sibling, display_items);
        }
    }

    pub fn paint(&self) -> Vec<DisplayItem> {
        let mut display_items = Vec::new();
        Self::paint_node(&self.root, &mut display_items);
        display_items
    }

    pub fn root(&self) -> Option<Rc<RefCell<LayoutObject>>> {
        self.root.clone()
    }

    pub fn find_node_by_position(&self, position: (i64, i64)) -> Option<Rc<RefCell<LayoutObject>>> {
        Self::find_node_by_position_internal(&self.root(), position)
    }

    fn find_node_by_position_internal(
        node: &Option<Rc<RefCell<LayoutObject>>>,
        position: (i64, i64),
    ) -> Option<Rc<RefCell<LayoutObject>>> {
        match node {
            Some(n) => {
                let first_child = n.borrow().first_child();
                let result1 = Self::find_node_by_position_internal(&first_child, position);
                if result1.is_some() {
                    return result1;
                }

                let next_sibling = n.borrow().next_sibling();
                let result2 = Self::find_node_by_position_internal(&next_sibling, position);
                if result2.is_some() {
                    return result2;
                }

                if n.borrow().point().x() <= position.0
                    && position.0 <= (n.borrow().point().x() + n.borrow().size().width())
                    && n.borrow().point().y() <= position.1
                    && position.1 <= (n.borrow().point().y() + n.borrow().size().height())
                {
                    return Some(n.clone());
                }
                None
            }
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alloc::string::ToString;
    use crate::display_item::DisplayItem;
    use crate::renderer::css::cssom::CssParser;
    use crate::renderer::css::token::CssTokenizer;
    use crate::renderer::dom::api::get_style_content;
    use crate::renderer::dom::node::Element;
    use crate::renderer::dom::node::NodeKind;
    use crate::renderer::html::parser::HtmlParser;
    use crate::renderer::html::token::HtmlTokenizer;
    use alloc::string::String;
    use alloc::vec::Vec;

    fn create_layout_view(html: String, viewport_width: i64) -> LayoutView {
        let t = HtmlTokenizer::new(html);
        let window = HtmlParser::new(t).construct_tree();
        let dom = window.borrow().document();
        let style = get_style_content(dom.clone());
        let css_tokenizer = CssTokenizer::new(style);
        let cssom = CssParser::new(css_tokenizer).parse_stylesheet();
        LayoutView::new(dom, &cssom, viewport_width)
    }

    #[test]
    fn test_empty() {
        let layout_view = create_layout_view("".to_string(), 600);
        assert_eq!(None, layout_view.root());
    }

    #[test]
    fn test_body() {
        let html = "<html><head></head><body></body></html>".to_string();
        let layout_view = create_layout_view(html, 600);

        let root = layout_view.root();
        assert!(root.is_some());
        assert_eq!(
            LayoutObjectKind::Block,
            root.clone().expect("root should exist").borrow().kind()
        );
        assert_eq!(
            NodeKind::Element(Element::new("body", Vec::new())),
            root.clone()
                .expect("root should exist")
                .borrow()
                .node_kind()
        );
    }

    #[test]
    fn test_text() {
        let html = "<html><head></head><body>text</body></html>".to_string();
        let layout_view = create_layout_view(html, 600);

        let root = layout_view.root().expect("root should exist");
        let text = root.borrow().first_child();
        assert!(text.is_some());
        assert_eq!(
            LayoutObjectKind::Text,
            text.clone()
                .expect("text node should exist")
                .borrow()
                .kind()
        );
    }

    #[test]
    fn test_example_like_layout_keeps_heading_width() {
        let html = r#"<html><head><style>body{background:#eee;width:60vw;margin:15vh auto;font-family:system-ui,sans-serif}h1{font-size:1.5em}div{opacity:0.8}a:link,a:visited{color:#348}</style></head><body><div><h1>Example Domain</h1><p>This domain is for use in documentation examples without needing permission. Avoid use in operations.</p><p><a href="https://iana.org/domains/example">Learn more</a></p></div></body></html>"#.to_string();
        let layout_view = create_layout_view(html, 1200);

        let body = layout_view.root().expect("body should exist");
        let div = body.borrow().first_child().expect("div should exist");
        let h1 = div.borrow().first_child().expect("h1 should exist");
        let display_items = layout_view.paint();

        assert!(
            body.borrow().size().width() >= 700,
            "body width was {}",
            body.borrow().size().width()
        );
        assert!(
            body.borrow().point().x() >= 200,
            "body x was {}",
            body.borrow().point().x()
        );
        assert!(
            h1.borrow().size().width() >= 300,
            "h1 width was {}",
            h1.borrow().size().width()
        );
        assert!(display_items.iter().any(|item| matches!(
            item,
            DisplayItem::Text { text, .. } if text == "Example Domain"
        )));
        assert!(display_items.iter().any(|item| matches!(
            item,
            DisplayItem::Rect { style, .. } if style.background_color().code() == "#eeeeee"
        )));
        assert!(display_items.iter().any(|item| matches!(
            item,
            DisplayItem::Text { text, style, .. }
                if text == "Example Domain"
                    && style.font_family() == "system-ui"
                    && (style.opacity() - 0.8).abs() < f64::EPSILON
        )));
        assert!(display_items.iter().any(|item| matches!(
            item,
            DisplayItem::Text { text, style, .. } if text == "Learn more" && style.color().code() == "#334488"
        )));
    }

    #[test]
    fn test_spacing_shorthand_and_auto_center_block() {
        let html = r#"<html><head><style>body{width:400px;margin:10px auto 30px auto;padding:8px 20px}p{margin:0}</style></head><body><p>Spacing</p></body></html>"#.to_string();
        let layout_view = create_layout_view(html, 1000);

        let body = layout_view.root().expect("body should exist");
        let paragraph = body.borrow().first_child().expect("paragraph should exist");
        let text = paragraph.borrow().first_child().expect("text should exist");

        assert_eq!(body.borrow().point().x(), 300);
        assert_eq!(body.borrow().point().y(), 10);
        assert_eq!(text.borrow().point().x(), 320);
        assert_eq!(text.borrow().point().y(), 18);
    }

    #[test]
    fn test_spacing_shorthand_supports_left_auto_alignment() {
        let html = r#"<html><head><style>body{width:400px;margin:10px 25px 30px auto}</style></head><body><p>Spacing</p></body></html>"#.to_string();
        let layout_view = create_layout_view(html, 1000);

        let body = layout_view.root().expect("body should exist");

        assert_eq!(body.borrow().point().x(), 575);
        assert_eq!(body.borrow().point().y(), 10);
    }

    #[test]
    fn test_form_control_placeholders_paint() {
        let html = r#"<html><head></head><body><form><input placeholder="Email" /><button>Send</button><img alt="Hero" /></form></body></html>"#.to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();

        assert!(display_items
            .iter()
            .any(|item| matches!(item, DisplayItem::Rect { .. })));
        assert!(display_items.iter().any(|item| matches!(
            item,
            DisplayItem::Text { text, .. } if text == "Email" || text == "Hero"
        )));
    }
}
