use crate::renderer::html::attribute::Attribute;
use std::format;
use std::rc::Rc;
use std::rc::Weak;
use std::string::String;
use std::vec::Vec;
use std::cell::RefCell;
use std::fmt::Display;
use std::fmt::Formatter;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct Window {
    document: Rc<RefCell<Node>>,
}

impl Window {
    pub fn new() -> Self {
        let window = Self {
            document: Rc::new(RefCell::new(Node::new(NodeKind::Document))),
        };

        window
            .document
            .borrow_mut()
            .set_window(Rc::downgrade(&Rc::new(RefCell::new(window.clone()))));

        window
    }

    pub fn document(&self) -> Rc<RefCell<Node>> {
        self.document.clone()
    }
}

#[derive(Debug, Clone)]
pub struct Node {
    pub kind: NodeKind,
    window: Weak<RefCell<Window>>,
    parent: Weak<RefCell<Node>>,
    first_child: Option<Rc<RefCell<Node>>>,
    last_child: Weak<RefCell<Node>>,
    previous_sibling: Weak<RefCell<Node>>,
    next_sibling: Option<Rc<RefCell<Node>>>,
}

impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl Node {
    pub fn new(kind: NodeKind) -> Self {
        Self {
            kind,
            window: Weak::new(),
            parent: Weak::new(),
            first_child: None,
            last_child: Weak::new(),
            previous_sibling: Weak::new(),
            next_sibling: None,
        }
    }

    pub fn set_window(&mut self, window: Weak<RefCell<Window>>) {
        self.window = window;
    }

    pub fn set_parent(&mut self, parent: Weak<RefCell<Node>>) {
        self.parent = parent;
    }

    pub fn parent(&self) -> Weak<RefCell<Node>> {
        self.parent.clone()
    }

    pub fn set_first_child(&mut self, first_child: Option<Rc<RefCell<Node>>>) {
        self.first_child = first_child;
    }

    pub fn first_child(&self) -> Option<Rc<RefCell<Node>>> {
        self.first_child.as_ref().cloned()
    }

    pub fn set_last_child(&mut self, last_child: Weak<RefCell<Node>>) {
        self.last_child = last_child;
    }

    pub fn last_child(&self) -> Weak<RefCell<Node>> {
        self.last_child.clone()
    }

    pub fn set_previous_sibling(&mut self, previous_sibling: Weak<RefCell<Node>>) {
        self.previous_sibling = previous_sibling;
    }

    pub fn previous_sibling(&self) -> Weak<RefCell<Node>> {
        self.previous_sibling.clone()
    }

    pub fn set_next_sibling(&mut self, next_sibling: Option<Rc<RefCell<Node>>>) {
        self.next_sibling = next_sibling;
    }

    pub fn next_sibling(&self) -> Option<Rc<RefCell<Node>>> {
        self.next_sibling.as_ref().cloned()
    }

    pub fn kind(&self) -> NodeKind {
        self.kind.clone()
    }

    pub fn get_element(&self) -> Option<Element> {
        match self.kind {
            NodeKind::Document | NodeKind::Text(_) => None,
            NodeKind::Element(ref e) => Some(e.clone()),
        }
    }

    pub fn element_kind(&self) -> Option<ElementKind> {
        match self.kind {
            NodeKind::Document | NodeKind::Text(_) => None,
            NodeKind::Element(ref e) => Some(e.kind()),
        }
    }
}

#[derive(Debug, Clone)]
pub enum NodeKind {
    Document,
    Element(Element),
    Text(String),
}

impl PartialEq for NodeKind {
    fn eq(&self, other: &Self) -> bool {
        match &self {
            NodeKind::Document => matches!(other, NodeKind::Document),
            NodeKind::Element(e1) => match &other {
                NodeKind::Element(e2) => e1.kind == e2.kind,
                _ => false,
            },
            NodeKind::Text(_) => matches!(other, NodeKind::Text(_)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    kind: ElementKind,
    attributes: Vec<Attribute>,
}

impl Element {
    pub fn new(element_name: &str, attributes: Vec<Attribute>) -> Self {
        // Unknown/custom element names (e.g. `<meta>`, `<svg>`, `<noscript>`,
        // `<wix-image>` and the countless tags used by modern CMS/framework
        // pages) must not crash the parser.  HTML5 specifies that unknown
        // elements behave as `HTMLUnknownElement` and default to inline
        // display, so we fall back to `Span` — this preserves the document
        // tree shape for selector matching while keeping content visible.
        // Spec: HTML Living Standard §4.2.2 — Custom and unknown elements.
        // https://html.spec.whatwg.org/multipage/dom.html#htmlunknownelement
        let kind = ElementKind::from_str(element_name).unwrap_or(ElementKind::Span);
        Self { kind, attributes }
    }

    pub fn kind(&self) -> ElementKind {
        self.kind
    }

    /// HTML "metadata content" plus `<script>` — elements that generate no
    /// rendered box. Their entire subtree (notably large inline `<script>` and
    /// `<style>` text) must be excluded from the layout tree. Otherwise a page
    /// carrying hundreds of kilobytes of inline script text builds an enormous
    /// run of inline text layout and the engine effectively hangs.
    /// Spec: the UA stylesheet sets `display: none` on these.
    /// https://html.spec.whatwg.org/multipage/rendering.html#hidden-elements
    pub fn is_non_rendered_element(&self) -> bool {
        matches!(
            self.kind,
            ElementKind::Head
                | ElementKind::Link
                | ElementKind::Style
                | ElementKind::Script
                | ElementKind::Title
        )
    }

    pub fn is_block_element(&self) -> bool {
        matches!(
            self.kind,
            ElementKind::Body
                | ElementKind::Div
                | ElementKind::Form
                | ElementKind::H1
                | ElementKind::H2
                | ElementKind::H3
                | ElementKind::Header
                | ElementKind::Li
                | ElementKind::Main
                | ElementKind::P
                | ElementKind::Section
                | ElementKind::Ul
                | ElementKind::Center
                | ElementKind::Table
                | ElementKind::Tr
                | ElementKind::Hr
                | ElementKind::Pre
                | ElementKind::Blockquote
                | ElementKind::Br
                | ElementKind::Td
                | ElementKind::Th
                | ElementKind::Dl
                | ElementKind::Dt
                | ElementKind::Dd
                | ElementKind::Caption
                | ElementKind::Tbody
                | ElementKind::Thead
                | ElementKind::Tfoot
        )
    }

    pub fn attributes(&self) -> Vec<Attribute> {
        self.attributes.clone()
    }

    pub fn get_attribute(&self, name: &str) -> Option<String> {
        for attr in &self.attributes {
            if attr.name() == name {
                return Some(attr.value());
            }
        }
        None
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ElementKind {
    Html,
    Head,
    Link,
    Style,
    Script,
    Title,
    Body,
    Div,
    Form,
    Span,
    Img,
    Input,
    Button,
    P,
    H1,
    H2,
    A,
    Ul,
    Li,
    Header,
    Main,
    Section,
    Br,
    Center,
    Table,
    Tr,
    Td,
    Th,
    Font,
    B,
    I,
    Strong,
    Em,
    Hr,
    Pre,
    Blockquote,
    Dl,
    Dt,
    Dd,
    H3,
    Caption,
    Tbody,
    Thead,
    Tfoot,
    Colgroup,
    Col,
}

impl Display for ElementKind {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        let s = match self {
            ElementKind::Html => "html",
            ElementKind::Head => "head",
            ElementKind::Link => "link",
            ElementKind::Style => "style",
            ElementKind::Script => "script",
            ElementKind::Title => "title",
            ElementKind::Body => "body",
            ElementKind::Div => "div",
            ElementKind::Form => "form",
            ElementKind::Span => "span",
            ElementKind::Img => "img",
            ElementKind::Input => "input",
            ElementKind::Button => "button",
            ElementKind::P => "p",
            ElementKind::H1 => "h1",
            ElementKind::H2 => "h2",
            ElementKind::A => "a",
            ElementKind::Ul => "ul",
            ElementKind::Li => "li",
            ElementKind::Header => "header",
            ElementKind::Main => "main",
            ElementKind::Section => "section",
            ElementKind::Br => "br",
            ElementKind::Center => "center",
            ElementKind::Table => "table",
            ElementKind::Tr => "tr",
            ElementKind::Td => "td",
            ElementKind::Th => "th",
            ElementKind::Font => "font",
            ElementKind::B => "b",
            ElementKind::I => "i",
            ElementKind::Strong => "strong",
            ElementKind::Em => "em",
            ElementKind::Hr => "hr",
            ElementKind::Pre => "pre",
            ElementKind::Blockquote => "blockquote",
            ElementKind::Dl => "dl",
            ElementKind::Dt => "dt",
            ElementKind::Dd => "dd",
            ElementKind::H3 => "h3",
            ElementKind::Caption => "caption",
            ElementKind::Tbody => "tbody",
            ElementKind::Thead => "thead",
            ElementKind::Tfoot => "tfoot",
            ElementKind::Colgroup => "colgroup",
            ElementKind::Col => "col",
        };
        write!(f, "{}", s)
    }
}

impl FromStr for ElementKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "html" => Ok(ElementKind::Html),
            "head" => Ok(ElementKind::Head),
            "link" => Ok(ElementKind::Link),
            "style" => Ok(ElementKind::Style),
            "script" => Ok(ElementKind::Script),
            "title" => Ok(ElementKind::Title),
            "body" => Ok(ElementKind::Body),
            "div" => Ok(ElementKind::Div),
            "form" => Ok(ElementKind::Form),
            "span" => Ok(ElementKind::Span),
            "img" => Ok(ElementKind::Img),
            "input" => Ok(ElementKind::Input),
            "button" => Ok(ElementKind::Button),
            "p" => Ok(ElementKind::P),
            "h1" => Ok(ElementKind::H1),
            "h2" => Ok(ElementKind::H2),
            "a" => Ok(ElementKind::A),
            "ul" => Ok(ElementKind::Ul),
            "li" => Ok(ElementKind::Li),
            "header" => Ok(ElementKind::Header),
            "main" => Ok(ElementKind::Main),
            "section" => Ok(ElementKind::Section),
            "br" => Ok(ElementKind::Br),
            "center" => Ok(ElementKind::Center),
            "table" => Ok(ElementKind::Table),
            "tr" => Ok(ElementKind::Tr),
            "td" => Ok(ElementKind::Td),
            "th" => Ok(ElementKind::Th),
            "font" => Ok(ElementKind::Font),
            "b" => Ok(ElementKind::B),
            "i" => Ok(ElementKind::I),
            "strong" => Ok(ElementKind::Strong),
            "em" => Ok(ElementKind::Em),
            "hr" => Ok(ElementKind::Hr),
            "pre" => Ok(ElementKind::Pre),
            "blockquote" => Ok(ElementKind::Blockquote),
            "dl" => Ok(ElementKind::Dl),
            "dt" => Ok(ElementKind::Dt),
            "dd" => Ok(ElementKind::Dd),
            "h3" => Ok(ElementKind::H3),
            "caption" => Ok(ElementKind::Caption),
            "tbody" => Ok(ElementKind::Tbody),
            "thead" => Ok(ElementKind::Thead),
            "tfoot" => Ok(ElementKind::Tfoot),
            "colgroup" => Ok(ElementKind::Colgroup),
            "col" => Ok(ElementKind::Col),
            _ => Err(format!("unimplemented element name {:?}", s)),
        }
    }
}

