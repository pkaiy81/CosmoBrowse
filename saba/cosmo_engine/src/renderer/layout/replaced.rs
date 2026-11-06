//! Replaced elements and form controls: boxes whose contents are not laid out
//! from the DOM. They size themselves from attributes and natural sizes, and
//! paint in `display::builder`.

use crate::renderer::dom::node::{Namespace, Node, NodeKind};
use crate::renderer::layout::computed_style::DisplayType;
use crate::renderer::layout::layout_object::{
    compute_box_model_metrics, LayoutObject, LayoutSize,
};
use crate::renderer::text::legacy_metrics::{measure_text_width, styled_line_height};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};

/// What's known about an image's own size, which only the renderer can find out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NaturalSize {
    Known(i64, i64),
    Failed,
}

#[derive(Default)]
struct ImageSizes {
    by_src: HashMap<String, NaturalSize>,
    generation: u64,
}

fn image_sizes() -> &'static Mutex<ImageSizes> {
    static SIZES: OnceLock<Mutex<ImageSizes>> = OnceLock::new();
    SIZES.get_or_init(Default::default)
}

/// Record that an image decoded at this size. Layout depends on it, so
/// [`image_sizes_generation`] moves when this tells it something new.
pub fn register_image_size(src: &str, width: u32, height: u32) {
    let mut sizes = image_sizes().lock().unwrap();
    let size = NaturalSize::Known(width as i64, height as i64);
    if sizes.by_src.insert(src.to_string(), size) != Some(size) {
        sizes.generation += 1;
    }
}

/// Record that an image can't be shown. Its alt text takes its place.
pub fn register_image_failed(src: &str) {
    let mut sizes = image_sizes().lock().unwrap();
    if sizes.by_src.insert(src.to_string(), NaturalSize::Failed) != Some(NaturalSize::Failed) {
        sizes.generation += 1;
    }
}

/// Moves whenever layout should run again to take in new image sizes.
pub fn image_sizes_generation() -> u64 {
    image_sizes().lock().unwrap().generation
}

fn natural_size(src: &str) -> Option<NaturalSize> {
    image_sizes().lock().unwrap().by_src.get(src).copied()
}

/// The kinds of `<input>`, by how they look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InputKind {
    Text,
    Password,
    Checkbox,
    Radio,
    Button,
    Range,
    Color,
    File,
}

impl LayoutObject {
    pub(crate) fn tag(&self) -> String {
        self.node.borrow().element_tag_name().unwrap_or_default()
    }

    /// Whether this box's contents aren't laid out from its children.
    pub(crate) fn is_replaced_box(&self) -> bool {
        let node = self.node.borrow();
        let NodeKind::Element(ref element) = node.kind else {
            return false;
        };
        match element.namespace() {
            Namespace::Svg => element.tag_name() == "svg",
            Namespace::MathMl => false,
            Namespace::Html => matches!(
                element.tag_name(),
                "img" | "input" | "select" | "progress" | "meter" | "video" | "audio" | "canvas"
                    | "iframe" | "embed" | "object"
            ),
        }
    }

    pub(crate) fn input_kind(&self) -> InputKind {
        let kind = self
            .element_attribute("type")
            .unwrap_or_default()
            .to_ascii_lowercase();
        match kind.as_str() {
            "password" => InputKind::Password,
            "checkbox" => InputKind::Checkbox,
            "radio" => InputKind::Radio,
            "submit" | "button" | "reset" | "image" => InputKind::Button,
            "range" => InputKind::Range,
            "color" => InputKind::Color,
            "file" => InputKind::File,
            _ => InputKind::Text,
        }
    }

    /// The label on a button-like `<input>`.
    pub(crate) fn input_button_label(&self) -> String {
        let kind = self.element_attribute("type").unwrap_or_default().to_ascii_lowercase();
        self.element_attribute("value").unwrap_or_else(|| {
            match kind.as_str() {
                "reset" => "Reset",
                "submit" => "Submit",
                "file" => "Choose File",
                _ => "",
            }
            .to_string()
        })
    }

    /// The source to show: from a `<picture>`, then `srcset`, then `src`.
    /// https://html.spec.whatwg.org/multipage/images.html#update-the-source-set
    pub(crate) fn current_image_src(&self) -> String {
        let node = self.node.borrow();
        let attribute = |n: &Node, name: &str| match n.kind {
            NodeKind::Element(ref e) => e.get_attribute(name),
            _ => None,
        };
        if let Some(parent) = node.parent().upgrade() {
            let parent = parent.borrow();
            if parent.element_tag_name().as_deref() == Some("picture") {
                let mut sibling = parent.first_child();
                while let Some(s) = sibling {
                    let source = s.borrow();
                    if source.element_tag_name().as_deref() == Some("source") {
                        let usable_type = attribute(&source, "type")
                            .is_none_or(|t| !t.contains("avif") && !t.contains("jxl"));
                        // Media conditions aren't evaluated here, so a source that
                        // has one is only a fallback for one that doesn't.
                        if usable_type && attribute(&source, "media").is_none() {
                            if let Some(set) = attribute(&source, "srcset") {
                                if let Some(best) = best_from_srcset(&set, self.size().width()) {
                                    return best;
                                }
                            }
                        }
                    }
                    let next = source.next_sibling();
                    drop(source);
                    sibling = next;
                }
            }
        }
        if let Some(set) = attribute(&node, "srcset") {
            if let Some(best) = best_from_srcset(&set, self.size().width()) {
                return best;
            }
        }
        attribute(&node, "src").unwrap_or_default()
    }

    /// An image's size: what's declared, with the rest from its natural size or
    /// proportions.
    fn image_size(&self, parent_size: LayoutSize) -> LayoutSize {
        let css_width = self.resolved_width_for(parent_size);
        let css_height = self.resolved_height_for(parent_size);
        let attr = |name: &str| {
            self.element_attribute(name)
                .filter(|v| !v.contains('%'))
                .and_then(|v| v.trim().trim_end_matches("px").parse::<f64>().ok())
                .map(|v| v as i64)
                .filter(|v| *v > 0)
        };
        let width = (css_width > 0).then_some(css_width).or_else(|| attr("width"));
        let height = (css_height > 0).then_some(css_height).or_else(|| attr("height"));
        let src = self.current_image_src();
        let natural = natural_size(&src);
        let ratio = match natural {
            Some(NaturalSize::Known(w, h)) if w > 0 && h > 0 => Some(w as f64 / h as f64),
            // The attributes still give proportions while the image is loading.
            _ => match (attr("width"), attr("height")) {
                (Some(w), Some(h)) => Some(w as f64 / h as f64),
                _ => None,
            },
        }
        .or_else(|| self.style.aspect_ratio());
        let metrics = compute_box_model_metrics(&self.style);
        let inner = metrics.inner_horizontal();
        // Limits apply to the width first, and the proportions follow.
        let limit = |w: i64| {
            if self.style.has_max_width() || self.style.has_size_limits() {
                (self.style.clamp_width(w + inner, parent_size.width()) - inner).max(0)
            } else {
                w
            }
        };
        match (width, height) {
            (Some(w), Some(h)) => LayoutSize::new(w, h),
            (Some(w), None) => {
                let w = limit(w);
                LayoutSize::new(w, ratio.map(|r| (w as f64 / r).round() as i64).unwrap_or(0))
            }
            (None, Some(h)) => {
                LayoutSize::new(ratio.map(|r| (h as f64 * r).round() as i64).unwrap_or(0), h)
            }
            (None, None) => match natural {
                Some(NaturalSize::Known(w, h)) => {
                    let limited = limit(w);
                    let height = if limited == w {
                        h
                    } else {
                        (limited as f64 / w.max(1) as f64 * h as f64).round() as i64
                    };
                    LayoutSize::new(limited, height)
                }
                // Broken: room for the alt text in a line.
                Some(NaturalSize::Failed) => {
                    let alt = self.element_attribute("alt").unwrap_or_default();
                    if alt.is_empty() {
                        LayoutSize::new(0, 0)
                    } else {
                        LayoutSize::new(
                            measure_text_width(&alt, self.style.font_size(), self.style.text_face()),
                            styled_line_height(&self.style),
                        )
                    }
                }
                // Not known yet.
                None => LayoutSize::new(0, 0),
            },
        }
    }

    /// The content-box size of a replaced box, if this is one.
    pub(crate) fn intrinsic_inline_size(&self, parent_size: LayoutSize) -> Option<LayoutSize> {
        if !self.is_replaced_box() {
            return None;
        }
        let tag = self.tag();
        let explicit_width = self.resolved_width_for(parent_size);
        let explicit_height = self.resolved_height_for(parent_size);
        let attribute = |name: &str, default: i64| {
            self.element_attribute(name)
                .filter(|v| !v.contains('%'))
                .and_then(|v| v.trim().trim_end_matches("px").parse::<f64>().ok())
                .map(|v| v as i64)
                .filter(|v| *v > 0)
                .unwrap_or(default)
        };
        let sized = |width: i64, height: i64| {
            LayoutSize::new(
                if explicit_width > 0 { explicit_width } else { width },
                if explicit_height > 0 { explicit_height } else { height },
            )
        };
        let (font_size, face) = (self.style.font_size(), self.style.text_face());
        let line = styled_line_height(&self.style);
        Some(match tag.as_str() {
            "img" => self.image_size(parent_size),
            "input" => match self.input_kind() {
                InputKind::Text | InputKind::Password => {
                    // `size` is in characters, 20 by default; browsers measure an
                    // average one.
                    let characters = attribute("size", 20);
                    let average = measure_text_width("x", font_size, face).max(1);
                    sized(characters * average + average, line)
                }
                InputKind::Checkbox | InputKind::Radio => sized(13, 13),
                InputKind::Range => sized(129, 16),
                InputKind::Color => sized(44, 21),
                InputKind::Button | InputKind::File => sized(
                    measure_text_width(&self.input_button_label(), font_size, face),
                    line,
                ),
            },
            "select" => {
                let widest = self
                    .select_options()
                    .iter()
                    .map(|(label, _)| measure_text_width(label, font_size, face))
                    .max()
                    .unwrap_or(0);
                sized(widest + 18, line)
            }
            "progress" => sized(160, 16),
            "meter" => sized(80, 16),
            "audio" => sized(300, 54),
            "video" | "canvas" | "iframe" | "embed" | "object" => {
                sized(attribute("width", 300), attribute("height", 150))
            }
            // SVG: declared size, then the viewBox's proportions.
            "svg" => {
                let view_box = self.element_attribute("viewBox").and_then(|v| {
                    let numbers: Vec<f64> = v
                        .split(|c: char| c.is_whitespace() || c == ',')
                        .filter_map(|n| n.parse().ok())
                        .collect();
                    (numbers.len() == 4 && numbers[2] > 0.0 && numbers[3] > 0.0)
                        .then(|| (numbers[2], numbers[3]))
                });
                let number = |name: &str| {
                    self.element_attribute(name)
                        .filter(|v| !v.contains('%'))
                        .and_then(|v| {
                            v.trim()
                                .trim_end_matches("px")
                                .parse::<f64>()
                                .ok()
                        })
                        .map(|v| v as i64)
                };
                let width = (explicit_width > 0).then_some(explicit_width).or(number("width"));
                let height =
                    (explicit_height > 0).then_some(explicit_height).or(number("height"));
                let ratio = view_box.map(|(w, h)| w / h);
                match (width, height) {
                    (Some(w), Some(h)) => LayoutSize::new(w, h),
                    (Some(w), None) => LayoutSize::new(
                        w,
                        ratio.map(|r| (w as f64 / r) as i64).unwrap_or(150),
                    ),
                    (None, Some(h)) => LayoutSize::new(
                        ratio.map(|r| (h as f64 * r) as i64).unwrap_or(300),
                        h,
                    ),
                    (None, None) => match view_box {
                        Some((w, h)) => LayoutSize::new(w as i64, h as i64),
                        None => LayoutSize::new(300, 150),
                    },
                }
            }
            _ => return None,
        })
    }

    /// Each `<option>` of this `<select>`, as (label, whether it's selected).
    pub fn select_options(&self) -> Vec<(String, Rc<RefCell<Node>>)> {
        fn walk(node: &Rc<RefCell<Node>>, out: &mut Vec<(String, Rc<RefCell<Node>>)>) {
            let mut child = node.borrow().first_child();
            while let Some(c) = child {
                match c.borrow().element_tag_name().as_deref() {
                    Some("option") => {
                        let label = match c.borrow().kind {
                            NodeKind::Element(ref e) => e.get_attribute("label"),
                            _ => None,
                        };
                        let label = label.unwrap_or_else(|| {
                            let mut text = String::new();
                            crate::renderer::dom::api::collect_text(
                                c.borrow().first_child(),
                                &mut text,
                            );
                            text.split_whitespace().collect::<Vec<_>>().join(" ")
                        });
                        out.push((label, c.clone()));
                    }
                    Some("optgroup") => walk(&c, out),
                    _ => {}
                }
                let next = c.borrow().next_sibling();
                child = next;
            }
        }
        let mut options = Vec::new();
        walk(&self.node, &mut options);
        options
    }

    /// Index of the selected option: the last one marked, or the first.
    pub(crate) fn selected_option(&self) -> Option<(String, usize)> {
        let options = self.select_options();
        let marked = options.iter().rposition(|(_, node)| {
            matches!(node.borrow().kind, NodeKind::Element(ref e) if e.get_attribute("selected").is_some())
        });
        let index = marked.unwrap_or(0);
        options.get(index).map(|(label, _)| (label.clone(), index))
    }

    /// Where a control's text starts, centered in its content box.
    pub(crate) fn control_text_origin(&self) -> (i64, i64) {
        let origin = self.content_origin();
        let height = self.content_size().height();
        let line = styled_line_height(&self.style);
        (origin.x(), origin.y() + ((height - line) / 2).max(0))
    }

    /// The rows of an open `<select>`'s list: each option's rect, top to bottom.
    pub fn select_popup_rows(&self) -> Vec<(i64, i64, i64, i64)> {
        let (font_size, face) = (self.style.font_size(), self.style.text_face());
        let options = self.select_options();
        let widest = options
            .iter()
            .map(|(label, _)| measure_text_width(label, font_size, face))
            .max()
            .unwrap_or(0);
        let width = (widest + 12).max(self.size().width());
        let row = styled_line_height(&self.style) + 4;
        let x = self.point().x();
        let top = self.point().y() + self.size().height();
        (0..options.len() as i64)
            .map(|i| (x, top + i * row, width, row))
            .collect()
    }

    pub(crate) fn is_block_level_replaced(&self) -> bool {
        self.is_replaced_box() && self.style.display() == DisplayType::Block
    }
}

/// The candidate from a `srcset` best suited to a box this wide: the narrowest
/// that's at least as wide, or the widest there is. Density descriptors pick 1x.
fn best_from_srcset(srcset: &str, width: i64) -> Option<String> {
    let mut by_width: Vec<(i64, String)> = Vec::new();
    let mut plain: Option<String> = None;
    for candidate in srcset.split(',') {
        let mut parts = candidate.split_whitespace();
        let Some(url) = parts.next() else { continue };
        match parts.next() {
            Some(descriptor) if descriptor.ends_with('w') => {
                if let Ok(w) = descriptor.trim_end_matches('w').parse::<i64>() {
                    by_width.push((w, url.to_string()));
                }
            }
            Some(descriptor) if descriptor.ends_with('x') => {
                if descriptor.trim_end_matches('x').parse::<f64>().ok() == Some(1.0)
                    || plain.is_none()
                {
                    plain = Some(url.to_string());
                }
            }
            _ => plain = Some(url.to_string()),
        }
    }
    by_width.sort();
    by_width
        .iter()
        .find(|(w, _)| *w >= width)
        .or(by_width.last())
        .map(|(_, url)| url.clone())
        .or(plain)
}

/// Serialize an inline `<svg>` for rasterizing: at this box's size, with the
/// color its `currentColor` means, and with what its `<use>` elements refer to.
pub(crate) fn serialize_inline_svg(
    node: &Rc<RefCell<Node>>,
    width: i64,
    height: i64,
    color: &str,
    fill: Option<&str>,
) -> String {
    let mut body = String::new();
    let mut references = Vec::new();
    serialize_svg_children(node, &mut body, &mut references);

    let mut root_attributes = String::new();
    let mut has_view_box = false;
    if let NodeKind::Element(ref element) = node.borrow().kind {
        for attribute in element.attributes() {
            let name = attribute.name();
            if matches!(name.as_str(), "width" | "height" | "style" | "class" | "id" | "xmlns") {
                continue;
            }
            if name == "viewBox" {
                has_view_box = true;
            }
            root_attributes.push_str(&format!(" {}=\"{}\"", name, escape_xml(&attribute.value())));
        }
    }
    if !has_view_box {
        // Without one, the content keeps its own scale.
        root_attributes.push_str(&format!(" viewBox=\"0 0 {width} {height}\""));
    }
    let fill_attribute = fill
        .map(|f| format!(" fill=\"{}\"", escape_xml(f)))
        .unwrap_or_default();

    // `<use href="#x">` usually points outside this `<svg>`, at a sprite sheet.
    let mut defs = String::new();
    let root = document_root(node);
    let mut seen: Vec<String> = Vec::new();
    while let Some(id) = references.pop() {
        if seen.contains(&id) {
            continue;
        }
        seen.push(id.clone());
        if let Some(target) =
            crate::renderer::dom::api::get_element_by_id(Some(root.clone()), &id)
        {
            let mut nested = Vec::new();
            serialize_svg_element(&target, &mut defs, &mut nested);
            references.append(&mut nested);
        }
    }
    let defs = if defs.is_empty() {
        String::new()
    } else {
        format!("<defs>{defs}</defs>")
    };
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{width}\" height=\"{height}\" color=\"{color}\"{fill_attribute}{root_attributes}>{defs}{body}</svg>"
    )
}

fn document_root(node: &Rc<RefCell<Node>>) -> Rc<RefCell<Node>> {
    let mut current = node.clone();
    loop {
        let parent = current.borrow().parent().upgrade();
        match parent {
            Some(p) => current = p,
            None => return current,
        }
    }
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn serialize_svg_children(
    node: &Rc<RefCell<Node>>,
    out: &mut String,
    references: &mut Vec<String>,
) {
    let mut child = node.borrow().first_child();
    while let Some(c) = child {
        serialize_svg_element(&c, out, references);
        let next = c.borrow().next_sibling();
        child = next;
    }
}

fn serialize_svg_element(
    node: &Rc<RefCell<Node>>,
    out: &mut String,
    references: &mut Vec<String>,
) {
    match node.borrow().kind {
        NodeKind::Text(ref text) => out.push_str(&escape_xml(text)),
        NodeKind::Element(ref element) => {
            out.push('<');
            out.push_str(element.tag_name());
            for attribute in element.attributes() {
                let name = attribute.name();
                let value = attribute.value();
                if matches!(name.as_str(), "href" | "xlink:href") {
                    if let Some(id) = value.strip_prefix('#') {
                        references.push(id.to_string());
                    }
                }
                out.push_str(&format!(" {}=\"{}\"", name, escape_xml(&value)));
            }
            out.push('>');
            serialize_svg_children(node, out, references);
            out.push_str("</");
            out.push_str(element.tag_name());
            out.push('>');
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srcset_picks_the_narrowest_wide_enough() {
        let set = "a.png 100w, b.png 400w, c.png 800w";
        assert_eq!(best_from_srcset(set, 300).as_deref(), Some("b.png"));
        assert_eq!(best_from_srcset(set, 2000).as_deref(), Some("c.png"));
    }

    #[test]
    fn srcset_density_descriptors_prefer_one_x() {
        assert_eq!(
            best_from_srcset("a.png 2x, b.png 1x", 0).as_deref(),
            Some("b.png")
        );
    }
}
