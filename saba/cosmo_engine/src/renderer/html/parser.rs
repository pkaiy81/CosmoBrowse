//! The HTML tree builder: every insertion mode of the standard, with the adoption
//! agency algorithm, foster parenting and foreign content.
//! https://html.spec.whatwg.org/multipage/parsing.html#tree-construction

use crate::renderer::dom::api::detach_node as detach;
use crate::renderer::dom::node::{Element, Namespace, Node, NodeKind, Window};
use crate::renderer::html::attribute::Attribute;
use crate::renderer::html::token::{HtmlToken, HtmlTokenizer, State};
use std::cell::RefCell;
use std::rc::Rc;
use std::string::String;
use std::string::ToString;
use std::vec::Vec;

type NodeRef = Rc<RefCell<Node>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertionMode {
    Initial,
    BeforeHtml,
    BeforeHead,
    InHead,
    InHeadNoscript,
    AfterHead,
    InBody,
    Text,
    InTable,
    InTableText,
    InCaption,
    InColumnGroup,
    InTableBody,
    InRow,
    InCell,
    InTemplate,
    AfterBody,
    InFrameset,
    AfterFrameset,
    AfterAfterBody,
    AfterAfterFrameset,
}

/// An entry in the list of active formatting elements.
#[derive(Debug, Clone)]
enum Formatting {
    Marker,
    Element {
        node: NodeRef,
        tag: String,
        attributes: Vec<Attribute>,
    },
}

/// Where a new node goes.
struct InsertionPlace {
    parent: NodeRef,
    /// Before this child; at the end when `None`.
    before: Option<NodeRef>,
}

#[derive(Debug, Clone)]
pub struct HtmlParser {
    window: Rc<RefCell<Window>>,
    mode: InsertionMode,
    original_mode: InsertionMode,
    template_modes: Vec<InsertionMode>,
    open_elements: Vec<NodeRef>,
    active_formatting: Vec<Formatting>,
    head_element: Option<NodeRef>,
    form_element: Option<NodeRef>,
    /// Whether a `<frameset>` could still replace the body.
    frameset_ok: bool,
    foster_parenting: bool,
    quirks: bool,
    /// Characters held while deciding whether table text needs foster parenting.
    pending_table_text: Vec<char>,
    /// The next newline is dropped, right after `<pre>`, `<listing>` or `<textarea>`.
    ignore_next_newline: bool,
    /// Set for a fragment: what the fragment is being parsed into.
    fragment_context: Option<NodeRef>,
    t: HtmlTokenizer,
}

fn tag_of(node: &NodeRef) -> String {
    node.borrow().element_tag_name().unwrap_or_default()
}

fn namespace_of(node: &NodeRef) -> Namespace {
    match node.borrow().kind {
        NodeKind::Element(ref e) => e.namespace(),
        _ => Namespace::Html,
    }
}

fn is_html(node: &NodeRef, tag: &str) -> bool {
    match node.borrow().kind {
        NodeKind::Element(ref e) => e.namespace() == Namespace::Html && e.tag_name() == tag,
        _ => false,
    }
}

fn is_html_any(node: &NodeRef, tags: &[&str]) -> bool {
    match node.borrow().kind {
        NodeKind::Element(ref e) => {
            e.namespace() == Namespace::Html && tags.contains(&e.tag_name())
        }
        _ => false,
    }
}

fn attribute_of(node: &NodeRef, name: &str) -> Option<String> {
    match node.borrow().kind {
        NodeKind::Element(ref e) => e.get_attribute(name),
        _ => None,
    }
}

const SPECIAL_HTML: &[&str] = &[
    "address", "applet", "area", "article", "aside", "base", "basefont", "bgsound", "blockquote",
    "body", "br", "button", "caption", "center", "col", "colgroup", "dd", "details", "dir", "div",
    "dl", "dt", "embed", "fieldset", "figcaption", "figure", "footer", "form", "frame", "frameset",
    "h1", "h2", "h3", "h4", "h5", "h6", "head", "header", "hgroup", "hr", "html", "iframe", "img",
    "input", "keygen", "li", "link", "listing", "main", "marquee", "menu", "meta", "nav",
    "noembed", "noframes", "noscript", "object", "ol", "p", "param", "plaintext", "pre", "script",
    "search", "section", "select", "source", "style", "summary", "table", "tbody", "td",
    "template", "textarea", "tfoot", "th", "thead", "title", "tr", "track", "ul", "wbr", "xmp",
];

fn is_special(node: &NodeRef) -> bool {
    let tag = tag_of(node);
    match namespace_of(node) {
        Namespace::Html => SPECIAL_HTML.contains(&tag.as_str()),
        Namespace::MathMl => matches!(
            tag.as_str(),
            "mi" | "mo" | "mn" | "ms" | "mtext" | "annotation-xml"
        ),
        Namespace::Svg => matches!(tag.as_str(), "foreignObject" | "desc" | "title"),
    }
}

fn is_formatting_tag(tag: &str) -> bool {
    matches!(
        tag,
        "a" | "b" | "big" | "code" | "em" | "font" | "i" | "nobr" | "s" | "small" | "strike"
            | "strong" | "tt" | "u"
    )
}

fn is_mathml_text_integration(node: &NodeRef) -> bool {
    namespace_of(node) == Namespace::MathMl
        && matches!(tag_of(node).as_str(), "mi" | "mo" | "mn" | "ms" | "mtext")
}

fn is_html_integration(node: &NodeRef) -> bool {
    let tag = tag_of(node);
    match namespace_of(node) {
        Namespace::Svg => matches!(tag.as_str(), "foreignObject" | "desc" | "title"),
        Namespace::MathMl => {
            tag == "annotation-xml"
                && attribute_of(node, "encoding").is_some_and(|encoding| {
                    let encoding = encoding.to_ascii_lowercase();
                    encoding == "text/html" || encoding == "application/xhtml+xml"
                })
        }
        Namespace::Html => false,
    }
}

/// SVG's camel-cased names, which the tokenizer has lowercased.
fn adjust_svg_tag(tag: &str) -> &str {
    match tag {
        "altglyph" => "altGlyph",
        "altglyphdef" => "altGlyphDef",
        "altglyphitem" => "altGlyphItem",
        "animatecolor" => "animateColor",
        "animatemotion" => "animateMotion",
        "animatetransform" => "animateTransform",
        "clippath" => "clipPath",
        "feblend" => "feBlend",
        "fecolormatrix" => "feColorMatrix",
        "fecomponenttransfer" => "feComponentTransfer",
        "fecomposite" => "feComposite",
        "feconvolvematrix" => "feConvolveMatrix",
        "fediffuselighting" => "feDiffuseLighting",
        "fedisplacementmap" => "feDisplacementMap",
        "fedistantlight" => "feDistantLight",
        "fedropshadow" => "feDropShadow",
        "feflood" => "feFlood",
        "fefunca" => "feFuncA",
        "fefuncb" => "feFuncB",
        "fefuncg" => "feFuncG",
        "fefuncr" => "feFuncR",
        "fegaussianblur" => "feGaussianBlur",
        "feimage" => "feImage",
        "femerge" => "feMerge",
        "femergenode" => "feMergeNode",
        "femorphology" => "feMorphology",
        "feoffset" => "feOffset",
        "fepointlight" => "fePointLight",
        "fespecularlighting" => "feSpecularLighting",
        "fespotlight" => "feSpotLight",
        "fetile" => "feTile",
        "feturbulence" => "feTurbulence",
        "foreignobject" => "foreignObject",
        "glyphref" => "glyphRef",
        "lineargradient" => "linearGradient",
        "radialgradient" => "radialGradient",
        "textpath" => "textPath",
        other => other,
    }
}

fn adjust_svg_attribute(name: &str) -> &str {
    match name {
        "attributename" => "attributeName",
        "attributetype" => "attributeType",
        "basefrequency" => "baseFrequency",
        "baseprofile" => "baseProfile",
        "calcmode" => "calcMode",
        "clippathunits" => "clipPathUnits",
        "diffuseconstant" => "diffuseConstant",
        "edgemode" => "edgeMode",
        "filterunits" => "filterUnits",
        "glyphref" => "glyphRef",
        "gradienttransform" => "gradientTransform",
        "gradientunits" => "gradientUnits",
        "kernelmatrix" => "kernelMatrix",
        "kernelunitlength" => "kernelUnitLength",
        "keypoints" => "keyPoints",
        "keysplines" => "keySplines",
        "keytimes" => "keyTimes",
        "lengthadjust" => "lengthAdjust",
        "limitingconeangle" => "limitingConeAngle",
        "markerheight" => "markerHeight",
        "markerunits" => "markerUnits",
        "markerwidth" => "markerWidth",
        "maskcontentunits" => "maskContentUnits",
        "maskunits" => "maskUnits",
        "numoctaves" => "numOctaves",
        "pathlength" => "pathLength",
        "patterncontentunits" => "patternContentUnits",
        "patterntransform" => "patternTransform",
        "patternunits" => "patternUnits",
        "pointsatx" => "pointsAtX",
        "pointsaty" => "pointsAtY",
        "pointsatz" => "pointsAtZ",
        "preservealpha" => "preserveAlpha",
        "preserveaspectratio" => "preserveAspectRatio",
        "primitiveunits" => "primitiveUnits",
        "refx" => "refX",
        "refy" => "refY",
        "repeatcount" => "repeatCount",
        "repeatdur" => "repeatDur",
        "requiredextensions" => "requiredExtensions",
        "requiredfeatures" => "requiredFeatures",
        "specularconstant" => "specularConstant",
        "specularexponent" => "specularExponent",
        "spreadmethod" => "spreadMethod",
        "startoffset" => "startOffset",
        "stddeviation" => "stdDeviation",
        "stitchtiles" => "stitchTiles",
        "surfacescale" => "surfaceScale",
        "systemlanguage" => "systemLanguage",
        "tablevalues" => "tableValues",
        "targetx" => "targetX",
        "targety" => "targetY",
        "textlength" => "textLength",
        "viewbox" => "viewBox",
        "viewtarget" => "viewTarget",
        "xchannelselector" => "xChannelSelector",
        "ychannelselector" => "yChannelSelector",
        "zoomandpan" => "zoomAndPan",
        other => other,
    }
}

fn adjust_foreign_attributes(attributes: &mut [Attribute], svg: bool) {
    for attribute in attributes {
        let name = attribute.name();
        let adjusted = if svg {
            adjust_svg_attribute(&name).to_string()
        } else if name == "definitionurl" {
            "definitionURL".to_string()
        } else {
            name.clone()
        };
        if adjusted != name {
            *attribute = Attribute::from_name_value(&adjusted, &attribute.value());
        }
    }
}

/// Start tags that break out of foreign content back into HTML.
fn breaks_out_of_foreign(tag: &str, attributes: &[Attribute]) -> bool {
    match tag {
        "b" | "big" | "blockquote" | "body" | "br" | "center" | "code" | "dd" | "div" | "dl"
        | "dt" | "em" | "embed" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "head" | "hr" | "i"
        | "img" | "li" | "listing" | "menu" | "meta" | "nobr" | "ol" | "p" | "pre" | "ruby" | "s"
        | "small" | "span" | "strong" | "strike" | "sub" | "sup" | "table" | "tt" | "u" | "ul"
        | "var" => true,
        "font" => attributes
            .iter()
            .any(|a| matches!(a.name().as_str(), "color" | "face" | "size")),
        _ => false,
    }
}

impl HtmlParser {
    pub fn new(t: HtmlTokenizer) -> Self {
        Self {
            window: Rc::new(RefCell::new(Window::new())),
            mode: InsertionMode::Initial,
            original_mode: InsertionMode::Initial,
            template_modes: Vec::new(),
            open_elements: Vec::new(),
            active_formatting: Vec::new(),
            head_element: None,
            form_element: None,
            frameset_ok: true,
            foster_parenting: false,
            quirks: false,
            pending_table_text: Vec::new(),
            ignore_next_newline: false,
            fragment_context: None,
            t,
        }
    }

    pub fn construct_tree(&mut self) -> Rc<RefCell<Window>> {
        while let Some(token) = self.t.next() {
            let done = token == HtmlToken::Eof;
            self.process(token);
            if done {
                break;
            }
        }
        // Whatever is still open is done with.
        self.open_elements.clear();
        self.window.clone()
    }

    /// Parse `html` as the contents of an element with this tag, for `innerHTML`.
    /// Returns the top-level nodes.
    /// https://html.spec.whatwg.org/multipage/parsing.html#parsing-html-fragments
    pub fn parse_fragment(context_tag: &str, html: String) -> Vec<NodeRef> {
        Self::parse_fragment_in(context_tag, Namespace::Html, html)
    }

    pub fn parse_fragment_in(
        context_tag: &str,
        namespace: Namespace,
        html: String,
    ) -> Vec<NodeRef> {
        let mut parser = Self::new(HtmlTokenizer::new(html));
        let context = Rc::new(RefCell::new(Node::new(NodeKind::Element(
            Element::new_in(context_tag, Vec::new(), namespace),
        ))));
        let state = if namespace != Namespace::Html {
            State::Data
        } else {
            match context_tag {
                "title" | "textarea" => State::Rcdata,
                "style" | "xmp" | "iframe" | "noembed" | "noframes" => State::Rawtext,
                "script" => State::ScriptData,
                "noscript" => State::Rawtext,
                "plaintext" => State::Plaintext,
                _ => State::Data,
            }
        };
        parser.t.set_state(state);
        let root = Rc::new(RefCell::new(Node::new(NodeKind::Element(Element::new(
            "html",
            Vec::new(),
        )))));
        parser.append_to_document(root.clone());
        parser.open_elements.push(root.clone());
        if context_tag == "template" && namespace == Namespace::Html {
            parser.template_modes.push(InsertionMode::InTemplate);
        }
        parser.fragment_context = Some(context);
        parser.reset_insertion_mode();
        while let Some(token) = parser.t.next() {
            let done = token == HtmlToken::Eof;
            parser.process(token);
            if done {
                break;
            }
        }
        let mut nodes = Vec::new();
        let mut child = root.borrow().first_child();
        while let Some(c) = child {
            child = c.borrow().next_sibling();
            nodes.push(c);
        }
        nodes
    }

    // ---- The tree ----------------------------------------------------------

    fn document(&self) -> NodeRef {
        self.window.borrow().document()
    }

    fn append_to_document(&self, node: NodeRef) {
        append_child(&self.document(), node);
    }

    fn current(&self) -> Option<NodeRef> {
        self.open_elements.last().cloned()
    }

    fn current_is(&self, tag: &str) -> bool {
        self.current().is_some_and(|n| is_html(&n, tag))
    }

    /// The current node, or the fragment's context when only the root is open.
    fn adjusted_current(&self) -> Option<NodeRef> {
        if self.fragment_context.is_some() && self.open_elements.len() == 1 {
            return self.fragment_context.clone();
        }
        self.current()
    }

    fn fragment_is_select(&self) -> bool {
        self.fragment_context
            .as_ref()
            .is_some_and(|context| is_html(context, "select"))
    }

    fn has_template_on_stack(&self) -> bool {
        self.open_elements.iter().any(|n| is_html(n, "template"))
    }

    /// https://html.spec.whatwg.org/multipage/parsing.html#appropriate-place-for-inserting-a-node
    fn insertion_place(&self, target: Option<NodeRef>) -> InsertionPlace {
        let target = target
            .or_else(|| self.current())
            .unwrap_or_else(|| self.document());
        if self.foster_parenting && is_html_any(&target, &["table", "tbody", "tfoot", "thead", "tr"]) {
            let last_template = self.open_elements.iter().rposition(|n| is_html(n, "template"));
            let last_table = self.open_elements.iter().rposition(|n| is_html(n, "table"));
            if let Some(template) = last_template {
                if last_table.is_none_or(|table| template > table) {
                    return InsertionPlace {
                        parent: self.open_elements[template].clone(),
                        before: None,
                    };
                }
            }
            let Some(table) = last_table else {
                return InsertionPlace {
                    parent: self.open_elements[0].clone(),
                    before: None,
                };
            };
            let table_node = self.open_elements[table].clone();
            if let Some(parent) = table_node.borrow().parent().upgrade() {
                return InsertionPlace {
                    parent,
                    before: Some(table_node.clone()),
                };
            }
            return InsertionPlace {
                parent: self.open_elements[table - 1].clone(),
                before: None,
            };
        }
        InsertionPlace {
            parent: target,
            before: None,
        }
    }

    fn insert_at(&self, place: InsertionPlace, node: NodeRef) {
        match place.before {
            Some(before) => insert_before(&place.parent, node, &before),
            None => append_child(&place.parent, node),
        }
    }

    fn insert_comment(&mut self, text: String) {
        let node = new_node(NodeKind::Comment(text));
        let place = self.insertion_place(None);
        self.insert_at(place, node);
    }

    fn insert_char(&mut self, c: char) {
        let place = self.insertion_place(None);
        if matches!(place.parent.borrow().kind, NodeKind::Document) {
            return;
        }
        // Text joins the text node right before where it goes, if there is one.
        let previous = match &place.before {
            Some(before) => before.borrow().previous_sibling().upgrade(),
            None => place.parent.borrow().last_child().upgrade(),
        };
        if let Some(previous) = previous {
            if let NodeKind::Text(ref mut text) = previous.borrow_mut().kind {
                text.push(c);
                return;
            }
        }
        self.insert_at(place, new_node(NodeKind::Text(c.to_string())));
    }

    fn create_html_element(&self, tag: &str, attributes: Vec<Attribute>) -> NodeRef {
        new_node(NodeKind::Element(Element::new(tag, attributes)))
    }

    /// Insert an HTML element and push it on the stack.
    fn insert_element(&mut self, tag: &str, attributes: Vec<Attribute>) -> NodeRef {
        let node = self.create_html_element(tag, attributes);
        let place = self.insertion_place(None);
        self.insert_at(place, node.clone());
        self.open_elements.push(node.clone());
        node
    }

    fn insert_foreign_element(
        &mut self,
        tag: &str,
        attributes: Vec<Attribute>,
        namespace: Namespace,
    ) -> NodeRef {
        let node = new_node(NodeKind::Element(Element::new_in(tag, attributes, namespace)));
        let place = self.insertion_place(None);
        self.insert_at(place, node.clone());
        self.open_elements.push(node.clone());
        node
    }

    fn generic_text_element(&mut self, tag: &str, attributes: Vec<Attribute>, state: State) {
        self.insert_element(tag, attributes);
        self.t.set_state(state);
        self.original_mode = self.mode;
        self.mode = InsertionMode::Text;
    }

    fn pop_until(&mut self, tags: &[&str]) {
        while let Some(node) = self.open_elements.pop() {
            if is_html_any(&node, tags) {
                return;
            }
        }
    }

    fn pop_until_node(&mut self, target: &NodeRef) {
        while let Some(node) = self.open_elements.pop() {
            if Rc::ptr_eq(&node, target) {
                return;
            }
        }
    }

    fn remove_from_stack(&mut self, target: &NodeRef) {
        if let Some(index) = self.open_elements.iter().position(|n| Rc::ptr_eq(n, target)) {
            self.open_elements.remove(index);
        }
    }

    // ---- Scopes ------------------------------------------------------------

    /// https://html.spec.whatwg.org/multipage/parsing.html#has-an-element-in-the-specific-scope
    fn in_specific_scope(&self, tags: &[&str], boundary: &dyn Fn(&NodeRef) -> bool) -> bool {
        for node in self.open_elements.iter().rev() {
            if is_html_any(node, tags) {
                return true;
            }
            if boundary(node) {
                return false;
            }
        }
        false
    }

    fn scope_boundary(node: &NodeRef) -> bool {
        match namespace_of(node) {
            Namespace::Html => is_html_any(
                node,
                &["applet", "caption", "html", "table", "td", "th", "marquee", "object", "template"],
            ),
            Namespace::MathMl => matches!(
                tag_of(node).as_str(),
                "mi" | "mo" | "mn" | "ms" | "mtext" | "annotation-xml"
            ),
            Namespace::Svg => {
                matches!(tag_of(node).as_str(), "foreignObject" | "desc" | "title")
            }
        }
    }

    fn in_scope(&self, tags: &[&str]) -> bool {
        self.in_specific_scope(tags, &Self::scope_boundary)
    }

    fn in_list_item_scope(&self, tags: &[&str]) -> bool {
        self.in_specific_scope(tags, &|n| {
            Self::scope_boundary(n) || is_html_any(n, &["ol", "ul"])
        })
    }

    fn in_button_scope(&self, tags: &[&str]) -> bool {
        self.in_specific_scope(tags, &|n| Self::scope_boundary(n) || is_html(n, "button"))
    }

    fn in_table_scope(&self, tags: &[&str]) -> bool {
        self.in_specific_scope(tags, &|n| is_html_any(n, &["html", "table", "template"]))
    }

    fn node_in_scope(&self, target: &NodeRef) -> bool {
        for node in self.open_elements.iter().rev() {
            if Rc::ptr_eq(node, target) {
                return true;
            }
            if Self::scope_boundary(node) {
                return false;
            }
        }
        false
    }

    /// https://html.spec.whatwg.org/multipage/parsing.html#generate-implied-end-tags
    fn generate_implied_end_tags(&mut self, except: Option<&str>) {
        while let Some(node) = self.current() {
            let implied = is_html_any(
                &node,
                &["dd", "dt", "li", "optgroup", "option", "p", "rb", "rp", "rt", "rtc"],
            );
            if !implied || except.is_some_and(|tag| is_html(&node, tag)) {
                return;
            }
            self.open_elements.pop();
        }
    }

    fn generate_all_implied_end_tags(&mut self) {
        while let Some(node) = self.current() {
            let implied = is_html_any(
                &node,
                &[
                    "caption", "colgroup", "dd", "dt", "li", "optgroup", "option", "p", "rb",
                    "rp", "rt", "rtc", "tbody", "td", "tfoot", "th", "thead", "tr",
                ],
            );
            if !implied {
                return;
            }
            self.open_elements.pop();
        }
    }

    fn close_p(&mut self) {
        self.generate_implied_end_tags(Some("p"));
        self.pop_until(&["p"]);
    }

    fn close_p_if_in_button_scope(&mut self) {
        if self.in_button_scope(&["p"]) {
            self.close_p();
        }
    }

    // ---- Active formatting elements ----------------------------------------

    /// https://html.spec.whatwg.org/multipage/parsing.html#push-onto-the-list-of-active-formatting-elements
    fn push_active_formatting(&mut self, node: NodeRef, tag: &str, attributes: Vec<Attribute>) {
        // Noah's Ark: no more than three identical elements since the last marker.
        let same = |a: &[Attribute], b: &[Attribute]| {
            a.len() == b.len() && a.iter().all(|x| b.contains(x))
        };
        let mut matching = Vec::new();
        for (index, entry) in self.active_formatting.iter().enumerate().rev() {
            match entry {
                Formatting::Marker => break,
                Formatting::Element {
                    tag: t,
                    attributes: a,
                    ..
                } if t == tag && same(a, &attributes) => matching.push(index),
                _ => {}
            }
        }
        if matching.len() >= 3 {
            self.active_formatting.remove(matching[matching.len() - 1]);
        }
        self.active_formatting.push(Formatting::Element {
            node,
            tag: tag.to_string(),
            attributes,
        });
    }

    fn formatting_index_of(&self, target: &NodeRef) -> Option<usize> {
        self.active_formatting.iter().position(|entry| {
            matches!(entry, Formatting::Element { node, .. } if Rc::ptr_eq(node, target))
        })
    }

    /// https://html.spec.whatwg.org/multipage/parsing.html#reconstruct-the-active-formatting-elements
    fn reconstruct_active_formatting(&mut self) {
        let Some(last) = self.active_formatting.last() else {
            return;
        };
        let is_open = |parser: &Self, entry: &Formatting| match entry {
            Formatting::Marker => true,
            Formatting::Element { node, .. } => {
                parser.open_elements.iter().any(|n| Rc::ptr_eq(n, node))
            }
        };
        if is_open(self, last) {
            return;
        }
        let mut index = self.active_formatting.len() - 1;
        while index > 0 && !is_open(self, &self.active_formatting[index - 1]) {
            index -= 1;
        }
        for i in index..self.active_formatting.len() {
            let Formatting::Element { tag, attributes, .. } = self.active_formatting[i].clone()
            else {
                continue;
            };
            let node = self.insert_element(&tag, attributes.clone());
            self.active_formatting[i] = Formatting::Element {
                node,
                tag,
                attributes,
            };
        }
    }

    fn clear_active_formatting_to_marker(&mut self) {
        while let Some(entry) = self.active_formatting.pop() {
            if matches!(entry, Formatting::Marker) {
                return;
            }
        }
    }

    /// The adoption agency algorithm, for misnested formatting.
    /// https://html.spec.whatwg.org/multipage/parsing.html#adoption-agency-algorithm
    fn adoption_agency(&mut self, subject: &str) -> bool {
        if let Some(current) = self.current() {
            if is_html(&current, subject) && self.formatting_index_of(&current).is_none() {
                self.open_elements.pop();
                return true;
            }
        }
        for _ in 0..8 {
            // The last formatting element with this tag since the last marker.
            let mut found = None;
            for (index, entry) in self.active_formatting.iter().enumerate().rev() {
                match entry {
                    Formatting::Marker => break,
                    Formatting::Element { tag, .. } if tag == subject => {
                        found = Some(index);
                        break;
                    }
                    _ => {}
                }
            }
            let Some(formatting_index) = found else {
                return false;
            };
            let Formatting::Element { node: formatting, .. } =
                self.active_formatting[formatting_index].clone()
            else {
                return false;
            };
            let Some(stack_index) = self
                .open_elements
                .iter()
                .position(|n| Rc::ptr_eq(n, &formatting))
            else {
                self.active_formatting.remove(formatting_index);
                return true;
            };
            if !self.node_in_scope(&formatting) {
                return true;
            }
            // The topmost special element below the formatting element.
            let furthest_block = self.open_elements[stack_index + 1..]
                .iter()
                .find(|n| is_special(n))
                .cloned();
            let Some(furthest_block) = furthest_block else {
                self.pop_until_node(&formatting);
                self.active_formatting.remove(formatting_index);
                return true;
            };
            let common_ancestor = self.open_elements[stack_index - 1].clone();
            let mut bookmark = formatting_index;
            let mut node = furthest_block.clone();
            let mut last_node = furthest_block.clone();
            let mut inner = 0;
            let mut node_index = self
                .open_elements
                .iter()
                .position(|n| Rc::ptr_eq(n, &node))
                .unwrap_or(0);
            loop {
                inner += 1;
                node_index -= 1;
                node = self.open_elements[node_index].clone();
                if Rc::ptr_eq(&node, &formatting) {
                    break;
                }
                let in_list = self.formatting_index_of(&node);
                if inner > 3 {
                    if let Some(index) = in_list {
                        self.active_formatting.remove(index);
                        if index < bookmark {
                            bookmark -= 1;
                        }
                    }
                }
                let Some(index) = self.formatting_index_of(&node) else {
                    self.open_elements.remove(node_index);
                    continue;
                };
                // Replace it with a fresh copy, in both lists.
                let Formatting::Element { tag, attributes, .. } =
                    self.active_formatting[index].clone()
                else {
                    continue;
                };
                let copy = self.create_html_element(&tag, attributes.clone());
                self.active_formatting[index] = Formatting::Element {
                    node: copy.clone(),
                    tag,
                    attributes,
                };
                self.open_elements[node_index] = copy.clone();
                node = copy;
                if Rc::ptr_eq(&last_node, &furthest_block) {
                    bookmark = index + 1;
                }
                detach(&last_node);
                append_child(&node, last_node.clone());
                last_node = node.clone();
            }
            detach(&last_node);
            let place = self.insertion_place(Some(common_ancestor));
            self.insert_at(place, last_node.clone());

            let Some(formatting_now) = self.formatting_index_of(&formatting) else {
                return true;
            };
            let Formatting::Element { tag, attributes, .. } =
                self.active_formatting[formatting_now].clone()
            else {
                return true;
            };
            let copy = self.create_html_element(&tag, attributes.clone());
            let mut child = furthest_block.borrow().first_child();
            while let Some(c) = child {
                child = c.borrow().next_sibling();
                detach(&c);
                append_child(&copy, c);
            }
            append_child(&furthest_block, copy.clone());

            self.active_formatting.remove(formatting_now);
            if formatting_now < bookmark {
                bookmark -= 1;
            }
            let bookmark = bookmark.min(self.active_formatting.len());
            self.active_formatting.insert(
                bookmark,
                Formatting::Element {
                    node: copy.clone(),
                    tag,
                    attributes,
                },
            );
            self.remove_from_stack(&formatting);
            let at = self
                .open_elements
                .iter()
                .position(|n| Rc::ptr_eq(n, &furthest_block))
                .map(|i| i + 1)
                .unwrap_or(self.open_elements.len());
            self.open_elements.insert(at, copy);
        }
        true
    }

    // ---- Insertion modes ---------------------------------------------------

    /// https://html.spec.whatwg.org/multipage/parsing.html#reset-the-insertion-mode-appropriately
    fn reset_insertion_mode(&mut self) {
        for index in (0..self.open_elements.len()).rev() {
            let last = index == 0;
            let node = if last {
                self.fragment_context
                    .clone()
                    .unwrap_or_else(|| self.open_elements[index].clone())
            } else {
                self.open_elements[index].clone()
            };
            let tag = tag_of(&node);
            if namespace_of(&node) == Namespace::Html {
                match tag.as_str() {
                    "td" | "th" if !last => {
                        self.mode = InsertionMode::InCell;
                        return;
                    }
                    "tr" => {
                        self.mode = InsertionMode::InRow;
                        return;
                    }
                    "tbody" | "thead" | "tfoot" => {
                        self.mode = InsertionMode::InTableBody;
                        return;
                    }
                    "caption" => {
                        self.mode = InsertionMode::InCaption;
                        return;
                    }
                    "colgroup" => {
                        self.mode = InsertionMode::InColumnGroup;
                        return;
                    }
                    "table" => {
                        self.mode = InsertionMode::InTable;
                        return;
                    }
                    "template" => {
                        self.mode = self
                            .template_modes
                            .last()
                            .copied()
                            .unwrap_or(InsertionMode::InTemplate);
                        return;
                    }
                    "head" if !last => {
                        self.mode = InsertionMode::InHead;
                        return;
                    }
                    "body" => {
                        self.mode = InsertionMode::InBody;
                        return;
                    }
                    "frameset" => {
                        self.mode = InsertionMode::InFrameset;
                        return;
                    }
                    "html" => {
                        self.mode = if self.head_element.is_none() {
                            InsertionMode::BeforeHead
                        } else {
                            InsertionMode::AfterHead
                        };
                        return;
                    }
                    _ => {}
                }
            }
            if last {
                self.mode = InsertionMode::InBody;
                return;
            }
        }
        self.mode = InsertionMode::InBody;
    }

    /// Route a token: foreign content has its own rules.
    /// https://html.spec.whatwg.org/multipage/parsing.html#tree-construction-dispatcher
    fn process(&mut self, token: HtmlToken) {
        if self.ignore_next_newline {
            self.ignore_next_newline = false;
            if token == HtmlToken::Char('\n') {
                return;
            }
        }
        if self.uses_html_rules(&token) {
            self.t.set_cdata_allowed(
                self.adjusted_current()
                    .is_some_and(|n| namespace_of(&n) != Namespace::Html),
            );
            self.process_in_mode(self.mode, token);
        } else {
            self.process_foreign(token);
        }
        self.t.set_cdata_allowed(
            self.adjusted_current()
                .is_some_and(|n| namespace_of(&n) != Namespace::Html),
        );
    }

    fn uses_html_rules(&self, token: &HtmlToken) -> bool {
        let Some(node) = self.adjusted_current() else {
            return true;
        };
        if namespace_of(&node) == Namespace::Html {
            return true;
        }
        if is_mathml_text_integration(&node) {
            match token {
                HtmlToken::StartTag { tag, .. } if tag != "mglyph" && tag != "malignmark" => {
                    return true;
                }
                HtmlToken::Char(_) => return true,
                _ => {}
            }
        }
        if namespace_of(&node) == Namespace::MathMl && tag_of(&node) == "annotation-xml" {
            if let HtmlToken::StartTag { tag, .. } = token {
                if tag == "svg" {
                    return true;
                }
            }
        }
        if is_html_integration(&node) {
            match token {
                HtmlToken::StartTag { .. } | HtmlToken::Char(_) => return true,
                _ => {}
            }
        }
        matches!(token, HtmlToken::Eof)
    }

    fn process_in_mode(&mut self, mode: InsertionMode, token: HtmlToken) {
        match mode {
            InsertionMode::Initial => self.in_initial(token),
            InsertionMode::BeforeHtml => self.in_before_html(token),
            InsertionMode::BeforeHead => self.in_before_head(token),
            InsertionMode::InHead => self.in_head(token),
            InsertionMode::InHeadNoscript => self.in_head_noscript(token),
            InsertionMode::AfterHead => self.in_after_head(token),
            InsertionMode::InBody => self.in_body(token),
            InsertionMode::Text => self.in_text(token),
            InsertionMode::InTable => self.in_table(token),
            InsertionMode::InTableText => self.in_table_text(token),
            InsertionMode::InCaption => self.in_caption(token),
            InsertionMode::InColumnGroup => self.in_column_group(token),
            InsertionMode::InTableBody => self.in_table_body(token),
            InsertionMode::InRow => self.in_row(token),
            InsertionMode::InCell => self.in_cell(token),
            InsertionMode::InTemplate => self.in_template(token),
            InsertionMode::AfterBody => self.in_after_body(token),
            InsertionMode::InFrameset => self.in_frameset(token),
            InsertionMode::AfterFrameset => self.in_after_frameset(token),
            InsertionMode::AfterAfterBody => self.in_after_after_body(token),
            InsertionMode::AfterAfterFrameset => self.in_after_after_frameset(token),
        }
    }

    fn in_initial(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::Char(c) if is_space(c) => {}
            HtmlToken::Comment(text) => {
                append_child(&self.document(), new_node(NodeKind::Comment(text)));
            }
            HtmlToken::Doctype {
                name,
                public_id,
                system_id,
                force_quirks,
            } => {
                self.quirks = force_quirks
                    || name.as_deref() != Some("html")
                    || doctype_means_quirks(public_id.as_deref(), system_id.as_deref());
                append_child(
                    &self.document(),
                    new_node(NodeKind::Doctype {
                        name: name.unwrap_or_default(),
                        public_id: public_id.unwrap_or_default(),
                        system_id: system_id.unwrap_or_default(),
                    }),
                );
                self.mode = InsertionMode::BeforeHtml;
            }
            other => {
                self.quirks = true;
                self.mode = InsertionMode::BeforeHtml;
                self.process_in_mode(self.mode, other);
            }
        }
    }

    fn in_before_html(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::Doctype { .. } => {}
            HtmlToken::Comment(text) => {
                append_child(&self.document(), new_node(NodeKind::Comment(text)));
            }
            HtmlToken::Char(c) if is_space(c) => {}
            HtmlToken::StartTag { ref tag, ref attributes, .. } if tag == "html" => {
                let node = self.create_html_element("html", attributes.clone());
                append_child(&self.document(), node.clone());
                self.open_elements.push(node);
                self.mode = InsertionMode::BeforeHead;
            }
            HtmlToken::EndTag { ref tag }
                if !matches!(tag.as_str(), "head" | "body" | "html" | "br") => {}
            other => {
                let node = self.create_html_element("html", Vec::new());
                append_child(&self.document(), node.clone());
                self.open_elements.push(node);
                self.mode = InsertionMode::BeforeHead;
                self.process_in_mode(self.mode, other);
            }
        }
    }

    fn in_before_head(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::Char(c) if is_space(c) => {}
            HtmlToken::Comment(text) => self.insert_comment(text),
            HtmlToken::Doctype { .. } => {}
            HtmlToken::StartTag { ref tag, .. } if tag == "html" => self.in_body(token),
            HtmlToken::StartTag { ref tag, ref attributes, .. } if tag == "head" => {
                let head = self.insert_element("head", attributes.clone());
                self.head_element = Some(head);
                self.mode = InsertionMode::InHead;
            }
            HtmlToken::EndTag { ref tag }
                if !matches!(tag.as_str(), "head" | "body" | "html" | "br") => {}
            other => {
                let head = self.insert_element("head", Vec::new());
                self.head_element = Some(head);
                self.mode = InsertionMode::InHead;
                self.process_in_mode(self.mode, other);
            }
        }
    }

    fn in_head(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::Char(c) if is_space(c) => self.insert_char(c),
            HtmlToken::Comment(text) => self.insert_comment(text),
            HtmlToken::Doctype { .. } => {}
            HtmlToken::StartTag { ref tag, .. } if tag == "html" => self.in_body(token),
            HtmlToken::StartTag { ref tag, ref attributes, .. }
                if matches!(tag.as_str(), "base" | "basefont" | "bgsound" | "link" | "meta") =>
            {
                self.insert_element(tag, attributes.clone());
                self.open_elements.pop();
            }
            HtmlToken::StartTag { ref tag, ref attributes, .. } if tag == "title" => {
                self.generic_text_element(tag, attributes.clone(), State::Rcdata);
            }
            HtmlToken::StartTag { ref tag, ref attributes, .. }
                if matches!(tag.as_str(), "noframes" | "style") =>
            {
                self.generic_text_element(tag, attributes.clone(), State::Rawtext);
            }
            HtmlToken::StartTag { ref tag, ref attributes, .. } if tag == "noscript" => {
                // Scripting is on: its contents are only for browsers without it.
                self.generic_text_element(tag, attributes.clone(), State::Rawtext);
            }
            HtmlToken::StartTag { ref tag, ref attributes, .. } if tag == "script" => {
                self.generic_text_element(tag, attributes.clone(), State::ScriptData);
            }
            HtmlToken::EndTag { ref tag } if tag == "head" => {
                self.open_elements.pop();
                self.mode = InsertionMode::AfterHead;
            }
            HtmlToken::EndTag { ref tag }
                if matches!(tag.as_str(), "body" | "html" | "br") =>
            {
                self.open_elements.pop();
                self.mode = InsertionMode::AfterHead;
                self.process_in_mode(self.mode, token);
            }
            HtmlToken::StartTag { ref tag, ref attributes, .. } if tag == "template" => {
                self.insert_element(tag, attributes.clone());
                self.active_formatting.push(Formatting::Marker);
                self.frameset_ok = false;
                self.mode = InsertionMode::InTemplate;
                self.template_modes.push(InsertionMode::InTemplate);
            }
            HtmlToken::EndTag { ref tag } if tag == "template" => {
                if !self.has_template_on_stack() {
                    return;
                }
                self.generate_all_implied_end_tags();
                self.pop_until(&["template"]);
                self.clear_active_formatting_to_marker();
                self.template_modes.pop();
                self.reset_insertion_mode();
            }
            HtmlToken::StartTag { ref tag, .. } if tag == "head" => {}
            HtmlToken::EndTag { .. } => {}
            other => {
                self.open_elements.pop();
                self.mode = InsertionMode::AfterHead;
                self.process_in_mode(self.mode, other);
            }
        }
    }

    fn in_head_noscript(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::Doctype { .. } => {}
            HtmlToken::StartTag { ref tag, .. } if tag == "html" => self.in_body(token),
            HtmlToken::EndTag { ref tag } if tag == "noscript" => {
                self.open_elements.pop();
                self.mode = InsertionMode::InHead;
            }
            HtmlToken::Char(c) if is_space(c) => self.in_head(token),
            HtmlToken::Comment(_) => self.in_head(token),
            HtmlToken::StartTag { ref tag, .. }
                if matches!(
                    tag.as_str(),
                    "basefont" | "bgsound" | "link" | "meta" | "noframes" | "style"
                ) =>
            {
                self.in_head(token)
            }
            HtmlToken::EndTag { ref tag } if tag == "br" => {
                self.open_elements.pop();
                self.mode = InsertionMode::InHead;
                self.process_in_mode(self.mode, token);
            }
            HtmlToken::StartTag { ref tag, .. } if matches!(tag.as_str(), "head" | "noscript") => {}
            HtmlToken::EndTag { .. } => {}
            other => {
                self.open_elements.pop();
                self.mode = InsertionMode::InHead;
                self.process_in_mode(self.mode, other);
            }
        }
    }

    fn in_after_head(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::Char(c) if is_space(c) => self.insert_char(c),
            HtmlToken::Comment(text) => self.insert_comment(text),
            HtmlToken::Doctype { .. } => {}
            HtmlToken::StartTag { ref tag, .. } if tag == "html" => self.in_body(token),
            HtmlToken::StartTag { ref tag, ref attributes, .. } if tag == "body" => {
                self.insert_element(tag, attributes.clone());
                self.frameset_ok = false;
                self.mode = InsertionMode::InBody;
            }
            HtmlToken::StartTag { ref tag, ref attributes, .. } if tag == "frameset" => {
                self.insert_element(tag, attributes.clone());
                self.mode = InsertionMode::InFrameset;
            }
            HtmlToken::StartTag { ref tag, .. }
                if matches!(
                    tag.as_str(),
                    "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "script"
                        | "style" | "template" | "title"
                ) =>
            {
                // These still belong in the head, however late they turn up.
                if let Some(head) = self.head_element.clone() {
                    self.open_elements.push(head.clone());
                    self.in_head(token);
                    self.remove_from_stack(&head);
                }
            }
            HtmlToken::EndTag { ref tag } if tag == "template" => self.in_head(token),
            HtmlToken::StartTag { ref tag, .. } if tag == "head" => {}
            HtmlToken::EndTag { ref tag }
                if !matches!(tag.as_str(), "body" | "html" | "br") => {}
            other => {
                self.insert_element("body", Vec::new());
                self.mode = InsertionMode::InBody;
                self.process_in_mode(self.mode, other);
            }
        }
    }

    fn in_body(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::Char('\0') => {}
            HtmlToken::Char(c) if is_space(c) => {
                self.reconstruct_active_formatting();
                self.insert_char(c);
            }
            HtmlToken::Char(c) => {
                self.reconstruct_active_formatting();
                self.insert_char(c);
                self.frameset_ok = false;
            }
            HtmlToken::Comment(text) => self.insert_comment(text),
            HtmlToken::Doctype { .. } => {}
            HtmlToken::StartTag { tag, self_closing, attributes } => {
                self.in_body_start_tag(tag, self_closing, attributes);
            }
            HtmlToken::EndTag { tag } => self.in_body_end_tag(tag),
            HtmlToken::Eof => {
                if !self.template_modes.is_empty() {
                    self.in_template(HtmlToken::Eof);
                }
            }
        }
    }

    fn in_body_start_tag(&mut self, tag: String, self_closing: bool, attributes: Vec<Attribute>) {
        match tag.as_str() {
            "html" => {
                if self.has_template_on_stack() {
                    return;
                }
                if let Some(root) = self.open_elements.first().cloned() {
                    add_missing_attributes(&root, attributes);
                }
            }
            "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "script" | "style"
            | "template" | "title" => self.in_head(HtmlToken::StartTag {
                tag,
                self_closing,
                attributes,
            }),
            "body" => {
                let body = self.open_elements.get(1).cloned();
                let Some(body) = body.filter(|b| is_html(b, "body")) else {
                    return;
                };
                if self.has_template_on_stack() {
                    return;
                }
                self.frameset_ok = false;
                add_missing_attributes(&body, attributes);
            }
            "frameset" => {
                let body = self.open_elements.get(1).cloned();
                let Some(body) = body.filter(|b| is_html(b, "body")) else {
                    return;
                };
                if !self.frameset_ok {
                    return;
                }
                detach(&body);
                self.open_elements.truncate(1);
                self.insert_element(&tag, attributes);
                self.mode = InsertionMode::InFrameset;
            }
            "address" | "article" | "aside" | "blockquote" | "center" | "details" | "dialog"
            | "dir" | "div" | "dl" | "fieldset" | "figcaption" | "figure" | "footer" | "header"
            | "hgroup" | "main" | "menu" | "nav" | "ol" | "p" | "search" | "section" | "summary"
            | "ul" => {
                self.close_p_if_in_button_scope();
                self.insert_element(&tag, attributes);
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.close_p_if_in_button_scope();
                if self
                    .current()
                    .is_some_and(|n| is_html_any(&n, &["h1", "h2", "h3", "h4", "h5", "h6"]))
                {
                    self.open_elements.pop();
                }
                self.insert_element(&tag, attributes);
            }
            "pre" | "listing" => {
                self.close_p_if_in_button_scope();
                self.insert_element(&tag, attributes);
                self.ignore_next_newline = true;
                self.frameset_ok = false;
            }
            "form" => {
                let in_template = self.has_template_on_stack();
                if self.form_element.is_some() && !in_template {
                    return;
                }
                self.close_p_if_in_button_scope();
                let form = self.insert_element(&tag, attributes);
                if !in_template {
                    self.form_element = Some(form);
                }
            }
            "li" | "dd" | "dt" => {
                self.frameset_ok = false;
                let closers: &[&str] = if tag == "li" { &["li"] } else { &["dd", "dt"] };
                for index in (0..self.open_elements.len()).rev() {
                    let node = self.open_elements[index].clone();
                    if is_html_any(&node, closers) {
                        let name = tag_of(&node);
                        self.generate_implied_end_tags(Some(&name));
                        self.pop_until(&[name.as_str()]);
                        break;
                    }
                    if is_special(&node) && !is_html_any(&node, &["address", "div", "p"]) {
                        break;
                    }
                }
                self.close_p_if_in_button_scope();
                self.insert_element(&tag, attributes);
            }
            "plaintext" => {
                self.close_p_if_in_button_scope();
                self.insert_element(&tag, attributes);
                self.t.set_state(State::Plaintext);
            }
            "button" => {
                if self.in_scope(&["button"]) {
                    self.generate_implied_end_tags(None);
                    self.pop_until(&["button"]);
                }
                self.reconstruct_active_formatting();
                self.insert_element(&tag, attributes);
                self.frameset_ok = false;
            }
            "a" => {
                // An `<a>` already open since the last marker closes first.
                let mut open_a = None;
                for entry in self.active_formatting.iter().rev() {
                    match entry {
                        Formatting::Marker => break,
                        Formatting::Element { tag, node, .. } if tag == "a" => {
                            open_a = Some(node.clone());
                            break;
                        }
                        _ => {}
                    }
                }
                if let Some(node) = open_a {
                    if !self.adoption_agency("a") {
                        self.any_other_end_tag("a");
                    }
                    if let Some(index) = self.formatting_index_of(&node) {
                        self.active_formatting.remove(index);
                    }
                    self.remove_from_stack(&node);
                }
                self.reconstruct_active_formatting();
                let node = self.insert_element(&tag, attributes.clone());
                self.push_active_formatting(node, &tag, attributes);
            }
            "b" | "big" | "code" | "em" | "font" | "i" | "s" | "small" | "strike" | "strong"
            | "tt" | "u" => {
                self.reconstruct_active_formatting();
                let node = self.insert_element(&tag, attributes.clone());
                self.push_active_formatting(node, &tag, attributes);
            }
            "nobr" => {
                self.reconstruct_active_formatting();
                if self.in_scope(&["nobr"]) {
                    if !self.adoption_agency("nobr") {
                        self.any_other_end_tag("nobr");
                    }
                    self.reconstruct_active_formatting();
                }
                let node = self.insert_element(&tag, attributes.clone());
                self.push_active_formatting(node, &tag, attributes);
            }
            "applet" | "marquee" | "object" => {
                self.reconstruct_active_formatting();
                self.insert_element(&tag, attributes);
                self.active_formatting.push(Formatting::Marker);
                self.frameset_ok = false;
            }
            "table" => {
                if !self.quirks {
                    self.close_p_if_in_button_scope();
                }
                self.insert_element(&tag, attributes);
                self.frameset_ok = false;
                self.mode = InsertionMode::InTable;
            }
            "area" | "br" | "embed" | "img" | "keygen" | "wbr" => {
                self.reconstruct_active_formatting();
                self.insert_element(&tag, attributes);
                self.open_elements.pop();
                self.frameset_ok = false;
            }
            "input" => {
                if self.fragment_is_select() {
                    return;
                }
                if self.in_scope(&["select"]) {
                    self.generate_implied_end_tags(None);
                    self.pop_until(&["select"]);
                }
                self.reconstruct_active_formatting();
                let hidden = attributes.iter().any(|a| {
                    a.name() == "type" && a.value().eq_ignore_ascii_case("hidden")
                });
                self.insert_element(&tag, attributes);
                self.open_elements.pop();
                if !hidden {
                    self.frameset_ok = false;
                }
            }
            "param" | "source" | "track" => {
                self.insert_element(&tag, attributes);
                self.open_elements.pop();
            }
            "hr" => {
                self.close_p_if_in_button_scope();
                if self.in_scope(&["select"]) {
                    self.generate_implied_end_tags(None);
                }
                self.insert_element(&tag, attributes);
                self.open_elements.pop();
                self.frameset_ok = false;
            }
            "image" => {
                self.in_body_start_tag("img".to_string(), self_closing, attributes);
            }
            "textarea" => {
                self.insert_element(&tag, attributes);
                self.ignore_next_newline = true;
                self.t.set_state(State::Rcdata);
                self.original_mode = self.mode;
                self.frameset_ok = false;
                self.mode = InsertionMode::Text;
            }
            "xmp" => {
                self.close_p_if_in_button_scope();
                self.reconstruct_active_formatting();
                self.frameset_ok = false;
                self.generic_text_element(&tag, attributes, State::Rawtext);
            }
            "iframe" => {
                self.frameset_ok = false;
                self.generic_text_element(&tag, attributes, State::Rawtext);
            }
            "noembed" | "noscript" => {
                self.generic_text_element(&tag, attributes, State::Rawtext);
            }
            // A select can hold most markup, so it has no insertion mode of its own.
            // https://html.spec.whatwg.org/multipage/parsing.html#parsing-main-inbody
            "select" => {
                if self.fragment_is_select() {
                    return;
                }
                if self.in_scope(&["select"]) {
                    self.generate_implied_end_tags(None);
                    self.pop_until(&["select"]);
                } else {
                    self.reconstruct_active_formatting();
                    self.insert_element(&tag, attributes);
                    self.frameset_ok = false;
                }
            }
            "option" => {
                if self.in_scope(&["select"]) {
                    self.generate_implied_end_tags(Some("optgroup"));
                } else if self.current_is("option") {
                    self.open_elements.pop();
                }
                self.reconstruct_active_formatting();
                self.insert_element(&tag, attributes);
            }
            "optgroup" => {
                if self.in_scope(&["select"]) {
                    self.generate_implied_end_tags(None);
                } else if self.current_is("option") {
                    self.open_elements.pop();
                }
                self.reconstruct_active_formatting();
                self.insert_element(&tag, attributes);
            }
            "rb" | "rtc" => {
                if self.in_scope(&["ruby"]) {
                    self.generate_implied_end_tags(None);
                }
                self.insert_element(&tag, attributes);
            }
            "rp" | "rt" => {
                if self.in_scope(&["ruby"]) {
                    self.generate_implied_end_tags(Some("rtc"));
                }
                self.insert_element(&tag, attributes);
            }
            "math" | "svg" => {
                self.reconstruct_active_formatting();
                let svg = tag == "svg";
                let mut attributes = attributes;
                adjust_foreign_attributes(&mut attributes, svg);
                let namespace = if svg { Namespace::Svg } else { Namespace::MathMl };
                self.insert_foreign_element(&tag, attributes, namespace);
                if self_closing {
                    self.open_elements.pop();
                }
            }
            "caption" | "col" | "colgroup" | "frame" | "head" | "tbody" | "td" | "tfoot" | "th"
            | "thead" | "tr" => {}
            _ => {
                self.reconstruct_active_formatting();
                self.insert_element(&tag, attributes);
            }
        }
    }

    fn in_body_end_tag(&mut self, tag: String) {
        match tag.as_str() {
            "template" => self.in_head(HtmlToken::EndTag { tag }),
            "body" => {
                if self.in_scope(&["body"]) {
                    self.mode = InsertionMode::AfterBody;
                }
            }
            "html" => {
                if self.in_scope(&["body"]) {
                    self.mode = InsertionMode::AfterBody;
                    self.process_in_mode(self.mode, HtmlToken::EndTag { tag });
                }
            }
            "address" | "article" | "aside" | "blockquote" | "button" | "center" | "details"
            | "dialog" | "dir" | "div" | "dl" | "fieldset" | "figcaption" | "figure" | "footer"
            | "header" | "hgroup" | "listing" | "main" | "menu" | "nav" | "ol" | "pre" | "search"
            | "section" | "select" | "summary" | "ul" => {
                if self.in_scope(&[tag.as_str()]) {
                    self.generate_implied_end_tags(None);
                    self.pop_until(&[tag.as_str()]);
                }
            }
            "form" => {
                if !self.has_template_on_stack() {
                    let node = self.form_element.take();
                    let Some(node) = node.filter(|n| self.node_in_scope(n)) else {
                        return;
                    };
                    self.generate_implied_end_tags(None);
                    self.remove_from_stack(&node);
                } else if self.in_scope(&["form"]) {
                    self.generate_implied_end_tags(None);
                    self.pop_until(&["form"]);
                }
            }
            "p" => {
                if !self.in_button_scope(&["p"]) {
                    self.insert_element("p", Vec::new());
                }
                self.close_p();
            }
            "li" => {
                if self.in_list_item_scope(&["li"]) {
                    self.generate_implied_end_tags(Some("li"));
                    self.pop_until(&["li"]);
                }
            }
            "dd" | "dt" => {
                if self.in_scope(&[tag.as_str()]) {
                    self.generate_implied_end_tags(Some(&tag));
                    self.pop_until(&[tag.as_str()]);
                }
            }
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let headings = ["h1", "h2", "h3", "h4", "h5", "h6"];
                if self.in_scope(&headings) {
                    self.generate_implied_end_tags(None);
                    self.pop_until(&headings);
                }
            }
            "a" | "b" | "big" | "code" | "em" | "font" | "i" | "nobr" | "s" | "small" | "strike"
            | "strong" | "tt" | "u" => {
                if !self.adoption_agency(&tag) {
                    self.any_other_end_tag(&tag);
                }
            }
            "applet" | "marquee" | "object" => {
                if self.in_scope(&[tag.as_str()]) {
                    self.generate_implied_end_tags(None);
                    self.pop_until(&[tag.as_str()]);
                    self.clear_active_formatting_to_marker();
                }
            }
            "br" => self.in_body_start_tag("br".to_string(), false, Vec::new()),
            _ => self.any_other_end_tag(&tag),
        }
    }

    fn any_other_end_tag(&mut self, tag: &str) {
        for index in (0..self.open_elements.len()).rev() {
            let node = self.open_elements[index].clone();
            if is_html(&node, tag) {
                self.generate_implied_end_tags(Some(tag));
                self.open_elements.truncate(index);
                return;
            }
            if is_special(&node) {
                return;
            }
        }
    }

    fn in_text(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::Char(c) => self.insert_char(c),
            HtmlToken::Eof => {
                self.open_elements.pop();
                self.mode = self.original_mode;
                self.process_in_mode(self.mode, HtmlToken::Eof);
            }
            HtmlToken::EndTag { .. } => {
                self.open_elements.pop();
                self.mode = self.original_mode;
            }
            _ => {}
        }
    }

    fn clear_to_table_context(&mut self) {
        while let Some(node) = self.current() {
            if is_html_any(&node, &["table", "template", "html"]) {
                return;
            }
            self.open_elements.pop();
        }
    }

    fn clear_to_table_body_context(&mut self) {
        while let Some(node) = self.current() {
            if is_html_any(&node, &["tbody", "tfoot", "thead", "template", "html"]) {
                return;
            }
            self.open_elements.pop();
        }
    }

    fn clear_to_row_context(&mut self) {
        while let Some(node) = self.current() {
            if is_html_any(&node, &["tr", "template", "html"]) {
                return;
            }
            self.open_elements.pop();
        }
    }

    fn in_table(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::Char(_)
                if self.current().is_some_and(|n| {
                    is_html_any(&n, &["table", "tbody", "template", "tfoot", "thead", "tr"])
                }) =>
            {
                self.pending_table_text.clear();
                self.original_mode = self.mode;
                self.mode = InsertionMode::InTableText;
                self.process_in_mode(self.mode, token);
            }
            HtmlToken::Comment(text) => self.insert_comment(text),
            HtmlToken::Doctype { .. } => {}
            HtmlToken::StartTag { ref tag, ref attributes, .. } => match tag.as_str() {
                "caption" => {
                    self.clear_to_table_context();
                    self.active_formatting.push(Formatting::Marker);
                    self.insert_element(tag, attributes.clone());
                    self.mode = InsertionMode::InCaption;
                }
                "colgroup" => {
                    self.clear_to_table_context();
                    self.insert_element(tag, attributes.clone());
                    self.mode = InsertionMode::InColumnGroup;
                }
                "col" => {
                    self.clear_to_table_context();
                    self.insert_element("colgroup", Vec::new());
                    self.mode = InsertionMode::InColumnGroup;
                    self.process_in_mode(self.mode, token);
                }
                "tbody" | "tfoot" | "thead" => {
                    self.clear_to_table_context();
                    self.insert_element(tag, attributes.clone());
                    self.mode = InsertionMode::InTableBody;
                }
                "td" | "th" | "tr" => {
                    self.clear_to_table_context();
                    self.insert_element("tbody", Vec::new());
                    self.mode = InsertionMode::InTableBody;
                    self.process_in_mode(self.mode, token);
                }
                "table" => {
                    if !self.in_table_scope(&["table"]) {
                        return;
                    }
                    self.pop_until(&["table"]);
                    self.reset_insertion_mode();
                    self.process_in_mode(self.mode, token);
                }
                "style" | "script" | "template" => self.in_head(token),
                "input" => {
                    let hidden = attributes.iter().any(|a| {
                        a.name() == "type" && a.value().eq_ignore_ascii_case("hidden")
                    });
                    if !hidden {
                        return self.in_table_anything_else(token);
                    }
                    self.insert_element(tag, attributes.clone());
                    self.open_elements.pop();
                }
                "form" => {
                    if self.has_template_on_stack() || self.form_element.is_some() {
                        return;
                    }
                    let form = self.insert_element(tag, attributes.clone());
                    self.form_element = Some(form);
                    self.open_elements.pop();
                }
                _ => self.in_table_anything_else(token),
            },
            HtmlToken::EndTag { ref tag } => match tag.as_str() {
                "table" => {
                    if !self.in_table_scope(&["table"]) {
                        return;
                    }
                    self.pop_until(&["table"]);
                    self.reset_insertion_mode();
                }
                "body" | "caption" | "col" | "colgroup" | "html" | "tbody" | "td" | "tfoot"
                | "th" | "thead" | "tr" => {}
                "template" => self.in_head(token),
                _ => self.in_table_anything_else(token),
            },
            HtmlToken::Eof => self.in_body(HtmlToken::Eof),
            other => self.in_table_anything_else(other),
        }
    }

    fn in_table_anything_else(&mut self, token: HtmlToken) {
        self.foster_parenting = true;
        self.in_body(token);
        self.foster_parenting = false;
    }

    fn in_table_text(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::Char('\0') => {}
            HtmlToken::Char(c) => self.pending_table_text.push(c),
            other => {
                let pending = std::mem::take(&mut self.pending_table_text);
                if pending.iter().any(|c| !is_space(*c)) {
                    for c in pending {
                        self.in_table_anything_else(HtmlToken::Char(c));
                    }
                } else {
                    for c in pending {
                        self.insert_char(c);
                    }
                }
                self.mode = self.original_mode;
                self.process_in_mode(self.mode, other);
            }
        }
    }

    fn close_caption(&mut self) -> bool {
        if !self.in_table_scope(&["caption"]) {
            return false;
        }
        self.generate_implied_end_tags(None);
        self.pop_until(&["caption"]);
        self.clear_active_formatting_to_marker();
        self.mode = InsertionMode::InTable;
        true
    }

    fn in_caption(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::EndTag { ref tag } if tag == "caption" => {
                self.close_caption();
            }
            HtmlToken::StartTag { ref tag, .. }
                if matches!(
                    tag.as_str(),
                    "caption" | "col" | "colgroup" | "tbody" | "td" | "tfoot" | "th" | "thead"
                        | "tr"
                ) =>
            {
                if self.close_caption() {
                    self.process_in_mode(self.mode, token);
                }
            }
            HtmlToken::EndTag { ref tag } if tag == "table" => {
                if self.close_caption() {
                    self.process_in_mode(self.mode, token);
                }
            }
            HtmlToken::EndTag { ref tag }
                if matches!(
                    tag.as_str(),
                    "body" | "col" | "colgroup" | "html" | "tbody" | "td" | "tfoot" | "th"
                        | "thead" | "tr"
                ) => {}
            other => self.in_body(other),
        }
    }

    fn in_column_group(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::Char(c) if is_space(c) => self.insert_char(c),
            HtmlToken::Comment(text) => self.insert_comment(text),
            HtmlToken::Doctype { .. } => {}
            HtmlToken::StartTag { ref tag, .. } if tag == "html" => self.in_body(token),
            HtmlToken::StartTag { ref tag, ref attributes, .. } if tag == "col" => {
                self.insert_element(tag, attributes.clone());
                self.open_elements.pop();
            }
            HtmlToken::EndTag { ref tag } if tag == "colgroup" => {
                if self.current_is("colgroup") {
                    self.open_elements.pop();
                    self.mode = InsertionMode::InTable;
                }
            }
            HtmlToken::EndTag { ref tag } if tag == "col" => {}
            HtmlToken::StartTag { ref tag, .. } if tag == "template" => self.in_head(token),
            HtmlToken::EndTag { ref tag } if tag == "template" => self.in_head(token),
            HtmlToken::Eof => self.in_body(HtmlToken::Eof),
            other => {
                if self.current_is("colgroup") {
                    self.open_elements.pop();
                    self.mode = InsertionMode::InTable;
                    self.process_in_mode(self.mode, other);
                }
            }
        }
    }

    fn in_table_body(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::StartTag { ref tag, ref attributes, .. } if tag == "tr" => {
                self.clear_to_table_body_context();
                self.insert_element(tag, attributes.clone());
                self.mode = InsertionMode::InRow;
            }
            HtmlToken::StartTag { ref tag, .. } if matches!(tag.as_str(), "th" | "td") => {
                self.clear_to_table_body_context();
                self.insert_element("tr", Vec::new());
                self.mode = InsertionMode::InRow;
                self.process_in_mode(self.mode, token);
            }
            HtmlToken::EndTag { ref tag } if matches!(tag.as_str(), "tbody" | "tfoot" | "thead") => {
                if self.in_table_scope(&[tag.as_str()]) {
                    self.clear_to_table_body_context();
                    self.open_elements.pop();
                    self.mode = InsertionMode::InTable;
                }
            }
            HtmlToken::StartTag { ref tag, .. }
                if matches!(
                    tag.as_str(),
                    "caption" | "col" | "colgroup" | "tbody" | "tfoot" | "thead"
                ) =>
            {
                self.close_table_body_and_reprocess(token);
            }
            HtmlToken::EndTag { ref tag } if tag == "table" => {
                self.close_table_body_and_reprocess(token);
            }
            HtmlToken::EndTag { ref tag }
                if matches!(
                    tag.as_str(),
                    "body" | "caption" | "col" | "colgroup" | "html" | "td" | "th" | "tr"
                ) => {}
            other => self.in_table(other),
        }
    }

    fn close_table_body_and_reprocess(&mut self, token: HtmlToken) {
        if self.in_table_scope(&["tbody", "thead", "tfoot"]) {
            self.clear_to_table_body_context();
            self.open_elements.pop();
            self.mode = InsertionMode::InTable;
            self.process_in_mode(self.mode, token);
        }
    }

    fn close_row(&mut self) -> bool {
        if !self.in_table_scope(&["tr"]) {
            return false;
        }
        self.clear_to_row_context();
        self.open_elements.pop();
        self.mode = InsertionMode::InTableBody;
        true
    }

    fn in_row(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::StartTag { ref tag, ref attributes, .. }
                if matches!(tag.as_str(), "th" | "td") =>
            {
                self.clear_to_row_context();
                self.insert_element(tag, attributes.clone());
                self.mode = InsertionMode::InCell;
                self.active_formatting.push(Formatting::Marker);
            }
            HtmlToken::EndTag { ref tag } if tag == "tr" => {
                self.close_row();
            }
            HtmlToken::StartTag { ref tag, .. }
                if matches!(
                    tag.as_str(),
                    "caption" | "col" | "colgroup" | "tbody" | "tfoot" | "thead" | "tr"
                ) =>
            {
                if self.close_row() {
                    self.process_in_mode(self.mode, token);
                }
            }
            HtmlToken::EndTag { ref tag } if tag == "table" => {
                if self.close_row() {
                    self.process_in_mode(self.mode, token);
                }
            }
            HtmlToken::EndTag { ref tag } if matches!(tag.as_str(), "tbody" | "tfoot" | "thead") => {
                if self.in_table_scope(&[tag.as_str()]) && self.close_row() {
                    self.process_in_mode(self.mode, token);
                }
            }
            HtmlToken::EndTag { ref tag }
                if matches!(
                    tag.as_str(),
                    "body" | "caption" | "col" | "colgroup" | "html" | "td" | "th"
                ) => {}
            other => self.in_table(other),
        }
    }

    fn close_cell(&mut self) {
        self.generate_implied_end_tags(None);
        self.pop_until(&["td", "th"]);
        self.clear_active_formatting_to_marker();
        self.mode = InsertionMode::InRow;
    }

    fn in_cell(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::EndTag { ref tag } if matches!(tag.as_str(), "td" | "th") => {
                if self.in_table_scope(&[tag.as_str()]) {
                    self.generate_implied_end_tags(None);
                    self.pop_until(&[tag.as_str()]);
                    self.clear_active_formatting_to_marker();
                    self.mode = InsertionMode::InRow;
                }
            }
            HtmlToken::StartTag { ref tag, .. }
                if matches!(
                    tag.as_str(),
                    "caption" | "col" | "colgroup" | "tbody" | "td" | "tfoot" | "th" | "thead"
                        | "tr"
                ) =>
            {
                if self.in_table_scope(&["td", "th"]) {
                    self.close_cell();
                    self.process_in_mode(self.mode, token);
                }
            }
            HtmlToken::EndTag { ref tag }
                if matches!(tag.as_str(), "body" | "caption" | "col" | "colgroup" | "html") => {}
            HtmlToken::EndTag { ref tag }
                if matches!(tag.as_str(), "table" | "tbody" | "tfoot" | "thead" | "tr") =>
            {
                if self.in_table_scope(&[tag.as_str()]) {
                    self.close_cell();
                    self.process_in_mode(self.mode, token);
                }
            }
            other => self.in_body(other),
        }
    }

    fn in_template(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::Char(_) | HtmlToken::Comment(_) | HtmlToken::Doctype { .. } => {
                self.in_body(token)
            }
            HtmlToken::StartTag { ref tag, .. }
                if matches!(
                    tag.as_str(),
                    "base" | "basefont" | "bgsound" | "link" | "meta" | "noframes" | "script"
                        | "style" | "template" | "title"
                ) =>
            {
                self.in_head(token)
            }
            HtmlToken::EndTag { ref tag } if tag == "template" => self.in_head(token),
            HtmlToken::StartTag { ref tag, .. } => {
                let next = match tag.as_str() {
                    "caption" | "colgroup" | "tbody" | "tfoot" | "thead" => InsertionMode::InTable,
                    "col" => InsertionMode::InColumnGroup,
                    "tr" => InsertionMode::InTableBody,
                    "td" | "th" => InsertionMode::InRow,
                    _ => InsertionMode::InBody,
                };
                self.template_modes.pop();
                self.template_modes.push(next);
                self.mode = next;
                self.process_in_mode(self.mode, token);
            }
            HtmlToken::EndTag { .. } => {}
            HtmlToken::Eof => {
                if !self.has_template_on_stack() {
                    return;
                }
                self.pop_until(&["template"]);
                self.clear_active_formatting_to_marker();
                self.template_modes.pop();
                self.reset_insertion_mode();
                self.process_in_mode(self.mode, HtmlToken::Eof);
            }
        }
    }

    fn in_after_body(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::Char(c) if is_space(c) => self.in_body(token),
            HtmlToken::Comment(text) => {
                // Goes on the root, after the body.
                if let Some(root) = self.open_elements.first().cloned() {
                    append_child(&root, new_node(NodeKind::Comment(text)));
                }
            }
            HtmlToken::Doctype { .. } => {}
            HtmlToken::StartTag { ref tag, .. } if tag == "html" => self.in_body(token),
            HtmlToken::EndTag { ref tag } if tag == "html" => {
                if self.fragment_context.is_none() {
                    self.mode = InsertionMode::AfterAfterBody;
                }
            }
            HtmlToken::Eof => {}
            other => {
                self.mode = InsertionMode::InBody;
                self.process_in_mode(self.mode, other);
            }
        }
    }

    fn in_frameset(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::Char(c) if is_space(c) => self.insert_char(c),
            HtmlToken::Comment(text) => self.insert_comment(text),
            HtmlToken::Doctype { .. } => {}
            HtmlToken::StartTag { ref tag, ref attributes, .. } => match tag.as_str() {
                "html" => self.in_body(token),
                "frameset" => {
                    self.insert_element(tag, attributes.clone());
                }
                "frame" => {
                    self.insert_element(tag, attributes.clone());
                    self.open_elements.pop();
                }
                "noframes" => self.in_head(token),
                _ => {}
            },
            HtmlToken::EndTag { ref tag } if tag == "frameset" => {
                if self.open_elements.len() > 1 {
                    self.open_elements.pop();
                }
                if self.fragment_context.is_none() && !self.current_is("frameset") {
                    self.mode = InsertionMode::AfterFrameset;
                }
            }
            _ => {}
        }
    }

    fn in_after_frameset(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::Char(c) if is_space(c) => self.insert_char(c),
            HtmlToken::Comment(text) => self.insert_comment(text),
            HtmlToken::StartTag { ref tag, .. } if tag == "html" => self.in_body(token),
            HtmlToken::StartTag { ref tag, .. } if tag == "noframes" => self.in_head(token),
            HtmlToken::EndTag { ref tag } if tag == "html" => {
                self.mode = InsertionMode::AfterAfterFrameset;
            }
            _ => {}
        }
    }

    fn in_after_after_body(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::Comment(text) => {
                append_child(&self.document(), new_node(NodeKind::Comment(text)));
            }
            HtmlToken::Doctype { .. } => self.in_body(token),
            HtmlToken::Char(c) if is_space(c) => self.in_body(token),
            HtmlToken::StartTag { ref tag, .. } if tag == "html" => self.in_body(token),
            HtmlToken::Eof => {}
            other => {
                self.mode = InsertionMode::InBody;
                self.process_in_mode(self.mode, other);
            }
        }
    }

    fn in_after_after_frameset(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::Comment(text) => {
                append_child(&self.document(), new_node(NodeKind::Comment(text)));
            }
            HtmlToken::Doctype { .. } => self.in_body(token),
            HtmlToken::Char(c) if is_space(c) => self.in_body(token),
            HtmlToken::StartTag { ref tag, .. } if tag == "html" => self.in_body(token),
            HtmlToken::StartTag { ref tag, .. } if tag == "noframes" => self.in_head(token),
            _ => {}
        }
    }

    /// https://html.spec.whatwg.org/multipage/parsing.html#parsing-main-inforeign
    fn process_foreign(&mut self, token: HtmlToken) {
        match token {
            HtmlToken::Char('\0') => self.insert_char('\u{fffd}'),
            HtmlToken::Char(c) if is_space(c) => self.insert_char(c),
            HtmlToken::Char(c) => {
                self.insert_char(c);
                self.frameset_ok = false;
            }
            HtmlToken::Comment(text) => self.insert_comment(text),
            HtmlToken::Doctype { .. } => {}
            HtmlToken::StartTag { ref tag, ref attributes, .. }
                if breaks_out_of_foreign(tag, attributes) =>
            {
                while let Some(node) = self.current() {
                    if is_mathml_text_integration(&node)
                        || is_html_integration(&node)
                        || namespace_of(&node) == Namespace::Html
                    {
                        break;
                    }
                    self.open_elements.pop();
                }
                self.process_in_mode(self.mode, token);
            }
            HtmlToken::StartTag { .. } => self.foreign_start_tag(token),
            HtmlToken::EndTag { ref tag } if tag == "br" || tag == "p" => {
                while let Some(node) = self.current() {
                    if is_mathml_text_integration(&node)
                        || is_html_integration(&node)
                        || namespace_of(&node) == Namespace::Html
                    {
                        break;
                    }
                    self.open_elements.pop();
                }
                self.process_in_mode(self.mode, token);
            }
            HtmlToken::EndTag { ref tag } => {
                let mut index = self.open_elements.len();
                while index > 0 {
                    index -= 1;
                    let node = self.open_elements[index].clone();
                    if index == 0 {
                        return;
                    }
                    if tag_of(&node).eq_ignore_ascii_case(tag) {
                        self.open_elements.truncate(index);
                        return;
                    }
                    if namespace_of(&self.open_elements[index - 1]) == Namespace::Html {
                        self.process_in_mode(self.mode, token);
                        return;
                    }
                }
            }
            HtmlToken::Eof => self.process_in_mode(self.mode, HtmlToken::Eof),
        }
    }

    fn foreign_start_tag(&mut self, token: HtmlToken) {
        let HtmlToken::StartTag { tag, self_closing, mut attributes } = token else {
            return;
        };
        let namespace = self
            .adjusted_current()
            .map(|n| namespace_of(&n))
            .unwrap_or(Namespace::Html);
        let svg = namespace == Namespace::Svg;
        let tag = if svg { adjust_svg_tag(&tag).to_string() } else { tag };
        adjust_foreign_attributes(&mut attributes, svg);
        self.insert_foreign_element(&tag, attributes, namespace);
        if self_closing {
            self.open_elements.pop();
        }
    }
}

fn is_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\u{c}' | '\r' | ' ')
}

/// Which DOCTYPEs put a document in quirks mode.
/// https://html.spec.whatwg.org/multipage/parsing.html#the-initial-insertion-mode
fn doctype_means_quirks(public_id: Option<&str>, system_id: Option<&str>) -> bool {
    let public = public_id.unwrap_or("").to_ascii_lowercase();
    let system = system_id.unwrap_or("").to_ascii_lowercase();
    const EXACT: &[&str] = &[
        "-//w3o//dtd w3 html strict 3.0//en//",
        "-/w3c/dtd html 4.0 transitional/en",
        "html",
    ];
    const PREFIXES: &[&str] = &[
        "+//silmaril//dtd html pro v0r11 19970101//",
        "-//as//dtd html 3.0 aswedit + extensions//",
        "-//advasoft ltd//dtd html 3.0 aswedit + extensions//",
        "-//ietf//dtd html 2.0",
        "-//ietf//dtd html 3",
        "-//ietf//dtd html level 0",
        "-//ietf//dtd html level 1",
        "-//ietf//dtd html level 2",
        "-//ietf//dtd html level 3",
        "-//ietf//dtd html strict",
        "-//ietf//dtd html//",
        "-//metrius//dtd metrius presentational//",
        "-//microsoft//dtd internet explorer 2.0",
        "-//microsoft//dtd internet explorer 3.0",
        "-//netscape comm. corp.//dtd html//",
        "-//netscape comm. corp.//dtd strict html//",
        "-//o'reilly and associates//dtd html",
        "-//softquad",
        "-//spyglass//dtd html 2.0 extended//",
        "-//sq//dtd html 2.0 hotmetal + extensions//",
        "-//sun microsystems corp.//dtd hotjava",
        "-//w3c//dtd html 3",
        "-//w3c//dtd html 4.0 frameset//",
        "-//w3c//dtd html 4.0 transitional//",
        "-//w3c//dtd w3 html//",
        "-//w3o//dtd w3 html 3.0//",
        "-//webtechs//dtd mozilla html",
    ];
    if EXACT.contains(&public.as_str())
        || PREFIXES.iter().any(|p| public.starts_with(p))
        || system == "http://www.ibm.com/data/dtd/v11/ibmxhtml1-transitional.dtd"
    {
        return true;
    }
    system_id.is_none()
        && (public.starts_with("-//w3c//dtd html 4.01 frameset//")
            || public.starts_with("-//w3c//dtd html 4.01 transitional//"))
}

fn new_node(kind: NodeKind) -> NodeRef {
    Rc::new(RefCell::new(Node::new(kind)))
}

fn add_missing_attributes(node: &NodeRef, attributes: Vec<Attribute>) {
    if let NodeKind::Element(ref mut element) = node.borrow_mut().kind {
        for attribute in attributes {
            if element.get_attribute(&attribute.name()).is_none() {
                element.set_attribute(&attribute.name(), &attribute.value());
            }
        }
    }
}

fn append_child(parent: &NodeRef, child: NodeRef) {
    let last = parent.borrow().last_child().upgrade();
    child.borrow_mut().set_parent(Rc::downgrade(parent));
    match last {
        Some(last) => {
            child.borrow_mut().set_previous_sibling(Rc::downgrade(&last));
            last.borrow_mut().set_next_sibling(Some(child.clone()));
        }
        None => parent.borrow_mut().set_first_child(Some(child.clone())),
    }
    parent.borrow_mut().set_last_child(Rc::downgrade(&child));
}

fn insert_before(parent: &NodeRef, child: NodeRef, before: &NodeRef) {
    let previous = before.borrow().previous_sibling().upgrade();
    child.borrow_mut().set_parent(Rc::downgrade(parent));
    child.borrow_mut().set_next_sibling(Some(before.clone()));
    before.borrow_mut().set_previous_sibling(Rc::downgrade(&child));
    match previous {
        Some(previous) => {
            child.borrow_mut().set_previous_sibling(Rc::downgrade(&previous));
            previous.borrow_mut().set_next_sibling(Some(child));
        }
        None => parent.borrow_mut().set_first_child(Some(child)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    fn parse(html: &str) -> NodeRef {
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        document
    }

    /// Children of `node`, as tag names and quoted text.
    fn outline(node: &NodeRef) -> String {
        let mut parts = Vec::new();
        let mut child = node.borrow().first_child();
        while let Some(c) = child {
            child = c.borrow().next_sibling();
            match c.borrow().kind {
                NodeKind::Element(ref e) => {
                    let inner = outline(&c);
                    if inner.is_empty() {
                        parts.push(e.tag_name().to_string());
                    } else {
                        parts.push(format!("{}({})", e.tag_name(), inner));
                    }
                }
                NodeKind::Text(ref t) => parts.push(format!("{t:?}")),
                _ => {}
            }
        }
        parts.join(" ")
    }

    #[test]
    fn test_empty() {
        assert_eq!(outline(&parse("")), "html(head body)");
    }

    #[test]
    fn test_body() {
        assert_eq!(outline(&parse("<html><head></head><body></body></html>")), "html(head body)");
    }

    #[test]
    fn test_text() {
        assert_eq!(outline(&parse("<body>text</body>")), "html(head body(\"text\"))");
    }

    #[test]
    fn implied_tags_appear() {
        assert_eq!(
            outline(&parse("<p>a<p>b<ul><li>c<li>d</ul>")),
            "html(head body(p(\"a\") p(\"b\") ul(li(\"c\") li(\"d\"))))"
        );
    }

    #[test]
    fn table_parts_are_implied() {
        assert_eq!(
            outline(&parse("<table><tr><td>a<td>b<tr><td>c</table>")),
            "html(head body(table(tbody(tr(td(\"a\") td(\"b\")) tr(td(\"c\"))))))"
        );
    }

    #[test]
    fn stray_table_text_is_foster_parented() {
        assert_eq!(
            outline(&parse("<table>x<tr><td>y</table>")),
            "html(head body(\"x\" table(tbody(tr(td(\"y\"))))))"
        );
    }

    #[test]
    fn misnested_formatting_goes_through_the_adoption_agency() {
        assert_eq!(
            outline(&parse("<p>1<b>2<i>3</b>4</i>5</p>")),
            "html(head body(p(\"1\" b(\"2\" i(\"3\")) i(\"4\") \"5\")))"
        );
        assert_eq!(
            outline(&parse("<a>1<div>2</a>3</div>")),
            "html(head body(a(\"1\") div(a(\"2\") \"3\")))"
        );
    }

    #[test]
    fn formatting_carries_across_blocks() {
        assert_eq!(
            outline(&parse("<b>a<p>b</p>c</b>d")),
            "html(head body(b(\"a\" p(\"b\") \"c\") \"d\"))"
        );
    }

    #[test]
    fn head_content_stays_in_the_head() {
        assert_eq!(
            outline(&parse("<title>t</title><p>x")),
            "html(head(title(\"t\")) body(p(\"x\")))"
        );
    }

    #[test]
    fn script_and_style_content_is_text() {
        assert_eq!(
            outline(&parse("<script>a<b>c</script><style>p>q{}</style>")),
            "html(head(script(\"a<b>c\") style(\"p>q{}\")) body)"
        );
    }

    #[test]
    fn svg_gets_its_namespace_and_case() {
        let document = parse("<svg viewbox='0 0 1 1'><lineargradient/><foreignobject><p>x</p></foreignobject></svg>");
        assert_eq!(
            outline(&document),
            "html(head body(svg(linearGradient foreignObject(p(\"x\")))))"
        );
        let body = document
            .borrow()
            .first_child()
            .and_then(|html| html.borrow().last_child().upgrade())
            .expect("body");
        let svg = body.borrow().first_child().expect("svg");
        assert_eq!(namespace_of(&svg), Namespace::Svg);
        assert_eq!(attribute_of(&svg, "viewBox").as_deref(), Some("0 0 1 1"));
    }

    #[test]
    fn a_block_tag_breaks_out_of_svg() {
        assert_eq!(
            outline(&parse("<svg><circle/><div>x</div>")),
            "html(head body(svg(circle) div(\"x\")))"
        );
    }

    #[test]
    fn frameset_replaces_the_body() {
        assert_eq!(
            outline(&parse("<frameset><frame src=a><frame src=b></frameset>")),
            "html(head frameset(frame frame))"
        );
    }

    #[test]
    fn fragment_parses_into_its_context() {
        let nodes = HtmlParser::parse_fragment("div", "<p>a<p>b".to_string());
        assert_eq!(nodes.len(), 2);
        let nodes = HtmlParser::parse_fragment("tr", "<td>a<td>b".to_string());
        assert_eq!(nodes.len(), 2);
        assert!(is_html(&nodes[0], "td"));
        let nodes = HtmlParser::parse_fragment("textarea", "<b>x".to_string());
        assert!(matches!(nodes[0].borrow().kind, NodeKind::Text(ref t) if t == "<b>x"));
    }

    #[test]
    fn newline_after_pre_is_dropped() {
        assert_eq!(
            outline(&parse("<pre>\na</pre><textarea>\nb</textarea>")),
            "html(head body(pre(\"a\") textarea(\"b\")))"
        );
    }

    #[test]
    fn test_form_controls() {
        let _ = vec![0];
        assert_eq!(
            outline(&parse("<form><input><button>b</button></form>")),
            "html(head body(form(input button(\"b\"))))"
        );
    }

    #[test]
    fn test_unknown_and_html5_elements_keep_their_real_tags() {
        assert_eq!(
            outline(&parse("<nav><x-widget>a</x-widget></nav>")),
            "html(head body(nav(x-widget(\"a\"))))"
        );
    }

    #[test]
    fn an_unknown_end_tag_closes_only_its_own_element() {
        assert_eq!(
            outline(&parse("<div><x-a><p>t</x-b></p></x-a></div>")),
            "html(head body(div(x-a(p(\"t\")))))"
        );
    }
}
