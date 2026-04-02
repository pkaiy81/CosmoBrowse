use crate::error::Error;
use crate::renderer::dom::node::ElementKind;
use crate::renderer::dom::node::Node;
use crate::renderer::dom::node::NodeKind;
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::string::ToString;
use core::cell::RefCell;

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct EdgeSize {
    top: f64,
    right: f64,
    bottom: f64,
    left: f64,
}

impl EdgeSize {
    pub fn zero() -> Self {
        Self {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        }
    }

    pub fn all(value: f64) -> Self {
        Self::from_values(value, value, value, value)
    }

    pub fn from_values(top: f64, right: f64, bottom: f64, left: f64) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    pub fn horizontal(&self) -> f64 {
        self.left + self.right
    }

    pub fn vertical(&self) -> f64 {
        self.top + self.bottom
    }

    pub fn top(&self) -> f64 {
        self.top
    }

    pub fn left(&self) -> f64 {
        self.left
    }

    pub fn right(&self) -> f64 {
        self.right
    }

    pub fn bottom(&self) -> f64 {
        self.bottom
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComputedStyle {
    background_color: Option<Color>,
    background_image: Option<String>,
    color: Option<Color>,
    display: Option<DisplayType>,
    font_family: Option<String>,
    font_size: Option<FontSize>,
    text_decoration: Option<TextDecoration>,
    opacity: Option<f64>,
    height: Option<f64>,
    height_ratio: Option<f64>,
    width: Option<f64>,
    width_ratio: Option<f64>,
    margin_left_auto: bool,
    margin_right_auto: bool,
    margin: Option<EdgeSize>,
    padding: Option<EdgeSize>,
    border: Option<EdgeSize>,
    position: Option<PositionType>,
    offset_top: Option<f64>,
    offset_left: Option<f64>,
    text_align: Option<TextAlign>,
    z_index: Option<i32>,
    overflow_clip: Option<bool>,
}

impl ComputedStyle {
    pub fn new() -> Self {
        Self {
            background_color: None,
            background_image: None,
            color: None,
            display: None,
            font_family: None,
            font_size: None,
            text_decoration: None,
            opacity: None,
            height: None,
            height_ratio: None,
            width: None,
            width_ratio: None,
            margin_left_auto: false,
            margin_right_auto: false,
            margin: None,
            padding: None,
            border: None,
            position: None,
            offset_top: None,
            offset_left: None,
            text_align: None,
            z_index: None,
            overflow_clip: None,
        }
    }

    pub fn defaulting(&mut self, node: &Rc<RefCell<Node>>, parent_style: Option<ComputedStyle>) {
        if let Some(parent_style) = parent_style {
            if self.background_color.is_none() && parent_style.background_color() != Color::white()
            {
                self.background_color = Some(parent_style.background_color());
            }
            if self.color.is_none() && parent_style.color() != Color::black() {
                self.color = Some(parent_style.color());
            }
            if self.font_family.is_none() {
                self.font_family = Some(parent_style.font_family());
            }
            if self.font_size.is_none() && parent_style.font_size() != FontSize::Medium {
                self.font_size = Some(parent_style.font_size());
            }
            if self.text_decoration.is_none()
                && parent_style.text_decoration() != TextDecoration::None
            {
                self.text_decoration = Some(parent_style.text_decoration());
            }
            // text-align is inherited.
            if self.text_align.is_none() && parent_style.text_align != Some(TextAlign::Left) {
                self.text_align = parent_style.text_align;
            }
            let parent_opacity = parent_style.opacity();
            if let Some(opacity) = self.opacity {
                self.opacity = Some((opacity * parent_opacity).clamp(0.0, 1.0));
            } else if parent_opacity < 1.0 {
                self.opacity = Some(parent_opacity);
            }
        }

        // Handle HTML bgcolor and text attributes (presentational hints).
        if self.background_color.is_none() {
            if let Some(bgcolor) = get_element_attribute(node, "bgcolor") {
                if let Some(color) = parse_html_color(&bgcolor) {
                    self.background_color = Some(color);
                }
            }
        }
        // Handle HTML <body background="..."> attribute for tiled background image.
        if self.background_image.is_none() {
            if let Some(bg) = get_element_attribute(node, "background") {
                if !bg.is_empty() {
                    self.background_image = Some(bg);
                }
            }
        }

        if self.color.is_none() {
            // <body text="..."> or <font color="...">
            let color_attr = get_element_attribute(node, "text")
                .or_else(|| get_element_attribute(node, "color"));
            if let Some(color_val) = color_attr {
                if let Some(color) = parse_html_color(&color_val) {
                    self.color = Some(color);
                }
            }
        }

        if self.background_color.is_none() {
            self.background_color = Some(match node.borrow().element_kind() {
                Some(ElementKind::Button) | Some(ElementKind::Img) | Some(ElementKind::Input) => {
                    Color::lightgray()
                }
                Some(ElementKind::Hr) => Color::gray(),
                Some(ElementKind::Body) => Color::white(),
                // Use transparent default so parent backgrounds (e.g. body bgcolor) show through.
                _ => Color::transparent(),
            });
        }
        if self.color.is_none() {
            if node.borrow().element_kind() == Some(ElementKind::A) {
                self.color = Some(Color::link_blue());
            } else {
                self.color = Some(Color::black());
            }
        }
        if self.display.is_none() {
            self.display = Some(DisplayType::default(node));
        }
        if self.font_family.is_none() {
            self.font_family = Some("serif".to_string());
        }
        if self.font_size.is_none() {
            self.font_size = Some(FontSize::default(node));
        }
        if self.text_decoration.is_none() {
            self.text_decoration = Some(TextDecoration::default(node));
        }
        if self.opacity.is_none() {
            self.opacity = Some(1.0);
        }
        if self.height.is_none() {
            self.height = Some(0.0);
        }
        if self.width.is_none() {
            self.width = Some(0.0);
        }
        // Handle HTML align attribute (presentational hint).
        if let Some(align) = get_element_attribute(node, "align") {
            if align.eq_ignore_ascii_case("center") {
                self.margin_left_auto = true;
                self.margin_right_auto = true;
                if self.text_align.is_none() {
                    self.text_align = Some(TextAlign::Center);
                }
            } else if align.eq_ignore_ascii_case("right") {
                if self.text_align.is_none() {
                    self.text_align = Some(TextAlign::Right);
                }
            }
        }
        // <center> tag implies text-align: center for children.
        if node.borrow().element_kind() == Some(ElementKind::Center) && self.text_align.is_none() {
            self.text_align = Some(TextAlign::Center);
        }
        // Block children of <center> should be horizontally centered (margin auto).
        {
            let parent_is_center = node
                .borrow()
                .parent()
                .upgrade()
                .map(|p| p.borrow().element_kind() == Some(ElementKind::Center))
                .unwrap_or(false);
            if parent_is_center {
                if let NodeKind::Element(ref e) = node.borrow().kind() {
                    if e.is_block_element() {
                        self.margin_left_auto = true;
                        self.margin_right_auto = true;
                    }
                }
            }
        }

        if self.margin.is_none() {
            if node.borrow().element_kind() == Some(ElementKind::Hr) {
                self.margin = Some(EdgeSize::from_values(8.0, 0.0, 8.0, 0.0));
            } else {
                self.margin = Some(EdgeSize::zero());
            }
        }
        if self.padding.is_none() {
            // Default padding-left for list containers (UA stylesheet).
            if node.borrow().element_kind() == Some(ElementKind::Ul) {
                self.padding = Some(EdgeSize::from_values(0.0, 0.0, 0.0, 40.0));
            } else {
                self.padding = Some(EdgeSize::zero());
            }
        }
        if self.position.is_none() {
            self.position = Some(PositionType::Static);
        }
        if self.border.is_none() {
            self.border = Some(EdgeSize::zero());
        }
        if self.offset_top.is_none() {
            self.offset_top = Some(0.0);
        }
        if self.offset_left.is_none() {
            self.offset_left = Some(0.0);
        }
        if self.text_align.is_none() {
            self.text_align = Some(TextAlign::Left);
        }
        if self.z_index.is_none() {
            self.z_index = Some(0);
        }
        if self.overflow_clip.is_none() {
            self.overflow_clip = Some(false);
        }
    }

    pub fn set_background_color(&mut self, color: Color) {
        self.background_color = Some(color);
    }

    pub fn background_color(&self) -> Color {
        self.background_color
            .clone()
            .expect("failed to access CSS property: background-color")
    }

    pub fn background_image(&self) -> Option<&str> {
        self.background_image.as_deref()
    }

    pub fn text_align(&self) -> TextAlign {
        self.text_align.unwrap_or(TextAlign::Left)
    }

    pub fn set_text_align(&mut self, text_align: TextAlign) {
        self.text_align = Some(text_align);
    }

    pub fn set_color(&mut self, color: Color) {
        self.color = Some(color);
    }

    pub fn color(&self) -> Color {
        self.color
            .clone()
            .expect("failed to access CSS property: color")
    }

    pub fn set_display(&mut self, display: DisplayType) {
        self.display = Some(display);
    }

    pub fn display(&self) -> DisplayType {
        self.display
            .expect("failed to access CSS property: display")
    }

    pub fn set_font_family(&mut self, font_family: String) {
        self.font_family = Some(font_family);
    }

    pub fn font_family(&self) -> String {
        self.font_family
            .clone()
            .expect("failed to access CSS property: font-family")
    }

    pub fn set_font_size(&mut self, font_size: FontSize) {
        self.font_size = Some(font_size);
    }

    pub fn font_size(&self) -> FontSize {
        self.font_size
            .expect("failed to access CSS property: font-size")
    }

    pub fn font_size_or_default(&self) -> FontSize {
        self.font_size.unwrap_or(FontSize::Medium)
    }
    pub fn set_text_decoration(&mut self, text_decoration: TextDecoration) {
        self.text_decoration = Some(text_decoration);
    }

    pub fn text_decoration(&self) -> TextDecoration {
        self.text_decoration
            .expect("failed to access CSS property: text-decoration")
    }

    pub fn set_opacity(&mut self, opacity: f64) {
        self.opacity = Some(opacity.clamp(0.0, 1.0));
    }

    pub fn opacity(&self) -> f64 {
        self.opacity
            .expect("failed to access CSS property: opacity")
    }

    pub fn set_height(&mut self, height: f64) {
        self.height = Some(height);
        self.height_ratio = None;
    }

    pub fn set_height_ratio(&mut self, ratio: f64) {
        self.height_ratio = Some(ratio);
        self.height = Some(0.0);
    }

    pub fn height(&self) -> f64 {
        self.height.expect("failed to access CSS property: height")
    }

    pub fn height_ratio(&self) -> Option<f64> {
        self.height_ratio
    }

    pub fn set_width(&mut self, width: f64) {
        self.width = Some(width);
        self.width_ratio = None;
    }

    pub fn set_width_ratio(&mut self, ratio: f64) {
        self.width_ratio = Some(ratio);
        self.width = Some(0.0);
    }

    pub fn width(&self) -> f64 {
        self.width.expect("failed to access CSS property: width")
    }

    pub fn width_ratio(&self) -> Option<f64> {
        self.width_ratio
    }

    pub fn set_margin_all(&mut self, value: f64) {
        self.margin = Some(EdgeSize::all(value));
    }

    pub fn set_margin(&mut self, margin: EdgeSize) {
        self.margin = Some(margin);
    }

    pub fn set_margin_left_auto(&mut self, enabled: bool) {
        self.margin_left_auto = enabled;
    }

    pub fn set_margin_right_auto(&mut self, enabled: bool) {
        self.margin_right_auto = enabled;
    }

    pub fn margin_left_auto(&self) -> bool {
        self.margin_left_auto
    }

    pub fn margin_right_auto(&self) -> bool {
        self.margin_right_auto
    }

    pub fn margin_horizontal_auto(&self) -> bool {
        self.margin_left_auto && self.margin_right_auto
    }

    pub fn margin(&self) -> EdgeSize {
        self.margin.expect("failed to access CSS property: margin")
    }

    /// Returns computed margin if already cascaded/defaulted, otherwise CSS initial value (0).
    /// Spec: CSS2.2 margin initial value is `0`.
    /// https://www.w3.org/TR/CSS22/box.html#margin-properties
    pub fn margin_or_default(&self) -> EdgeSize {
        self.margin.unwrap_or(EdgeSize::zero())
    }

    pub fn set_padding_all(&mut self, value: f64) {
        self.padding = Some(EdgeSize::all(value));
    }

    pub fn set_padding(&mut self, padding: EdgeSize) {
        self.padding = Some(padding);
    }

    pub fn padding(&self) -> EdgeSize {
        self.padding
            .expect("failed to access CSS property: padding")
    }

    pub fn set_border_all(&mut self, value: f64) {
        self.border = Some(EdgeSize::all(value));
    }

    pub fn set_border(&mut self, border: EdgeSize) {
        self.border = Some(border);
    }

    pub fn border(&self) -> EdgeSize {
        self.border.expect("failed to access CSS property: border")
    }

    pub fn set_position(&mut self, position: PositionType) {
        self.position = Some(position);
    }

    pub fn position(&self) -> PositionType {
        self.position
            .expect("failed to access CSS property: position")
    }

    pub fn set_offset_top(&mut self, top: f64) {
        self.offset_top = Some(top);
    }

    pub fn offset_top(&self) -> f64 {
        self.offset_top.expect("failed to access CSS property: top")
    }

    pub fn set_offset_left(&mut self, left: f64) {
        self.offset_left = Some(left);
    }

    pub fn offset_left(&self) -> f64 {
        self.offset_left
            .expect("failed to access CSS property: left")
    }

    pub fn set_z_index(&mut self, z_index: i32) {
        self.z_index = Some(z_index);
    }

    pub fn z_index(&self) -> i32 {
        self.z_index
            .expect("failed to access CSS property: z-index")
    }

    pub fn set_overflow_clip(&mut self, clip: bool) {
        self.overflow_clip = Some(clip);
    }

    pub fn overflow_clip(&self) -> bool {
        self.overflow_clip
            .expect("failed to access CSS property: overflow")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Color {
    name: Option<String>,
    code: String,
}

impl Color {
    pub fn from_name(name: &str) -> Result<Self, Error> {
        let code = match name {
            "black" => "#000000".to_string(),
            "silver" => "#c0c0c0".to_string(),
            "gray" => "#808080".to_string(),
            "white" => "#ffffff".to_string(),
            "maroon" => "#800000".to_string(),
            "red" => "#ff0000".to_string(),
            "purple" => "#800080".to_string(),
            "fuchsia" => "#ff00ff".to_string(),
            "green" => "#008000".to_string(),
            "lime" => "#00ff00".to_string(),
            "olive" => "#808000".to_string(),
            "yellow" => "#ffff00".to_string(),
            "navy" => "#000080".to_string(),
            "blue" => "#0000ff".to_string(),
            "teal" => "#008080".to_string(),
            "aqua" => "#00ffff".to_string(),
            "orange" => "#ffa500".to_string(),
            "lightgray" => "#d3d3d3".to_string(),
            "transparent" => "#00000000".to_string(),
            _ => {
                return Err(Error::UnexpectedInput(format!(
                    "color name {:?} is not supported yet",
                    name
                )));
            }
        };

        Ok(Self {
            name: Some(name.to_string()),
            code,
        })
    }

    pub fn from_code(code: &str) -> Result<Self, Error> {
        if code.chars().nth(0) != Some('#') {
            return Err(Error::UnexpectedInput(format!(
                "invalid color code: {}",
                code
            )));
        }

        let normalized = if code.len() == 4 {
            let mut expanded = String::from("#");
            for ch in code.chars().skip(1) {
                expanded.push(ch);
                expanded.push(ch);
            }
            expanded
        } else {
            code.to_string()
        };

        if normalized.len() != 7 {
            return Err(Error::UnexpectedInput(format!(
                "invalid color code: {}",
                code
            )));
        }

        if normalized.chars().skip(1).any(|ch| !ch.is_ascii_hexdigit()) {
            return Err(Error::UnexpectedInput(format!(
                "invalid color code: {}",
                code
            )));
        }

        let name = match normalized.as_str() {
            "#000000" => Some("black".to_string()),
            "#c0c0c0" => Some("silver".to_string()),
            "#808080" => Some("gray".to_string()),
            "#ffffff" => Some("white".to_string()),
            "#800000" => Some("maroon".to_string()),
            "#ff0000" => Some("red".to_string()),
            "#800080" => Some("purple".to_string()),
            "#ff00ff" => Some("fuchsia".to_string()),
            "#008000" => Some("green".to_string()),
            "#00ff00" => Some("lime".to_string()),
            "#808000" => Some("olive".to_string()),
            "#ffff00" => Some("yellow".to_string()),
            "#000080" => Some("navy".to_string()),
            "#0000ff" => Some("blue".to_string()),
            "#008080" => Some("teal".to_string()),
            "#00ffff" => Some("aqua".to_string()),
            "#ffa500" => Some("orange".to_string()),
            "#d3d3d3" => Some("lightgray".to_string()),
            _ => None,
        };

        Ok(Self {
            name,
            code: normalized,
        })
    }

    pub fn white() -> Self {
        Self {
            name: Some("white".to_string()),
            code: "#ffffff".to_string(),
        }
    }

    pub fn transparent() -> Self {
        Self {
            name: Some("transparent".to_string()),
            code: "#00000000".to_string(),
        }
    }

    pub fn black() -> Self {
        Self {
            name: Some("black".to_string()),
            code: "#000000".to_string(),
        }
    }

    pub fn link_blue() -> Self {
        Self {
            name: Some("blue".to_string()),
            code: "#0000ee".to_string(),
        }
    }

    pub fn gray() -> Self {
        Self {
            name: Some("gray".to_string()),
            code: "#808080".to_string(),
        }
    }

    pub fn lightgray() -> Self {
        Self {
            name: Some("lightgray".to_string()),
            code: "#d3d3d3".to_string(),
        }
    }

    pub fn code_u32(&self) -> u32 {
        u32::from_str_radix(self.code.trim_start_matches('#'), 16).unwrap()
    }

    pub fn code(&self) -> &str {
        &self.code
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum FontSize {
    Medium,
    XLarge,
    XXLarge,
}

impl FontSize {
    fn default(node: &Rc<RefCell<Node>>) -> Self {
        match &node.borrow().kind() {
            NodeKind::Element(element) => match element.kind() {
                ElementKind::H1 => FontSize::XXLarge,
                ElementKind::H2 => FontSize::XLarge,
                ElementKind::H3 => FontSize::XLarge,
                _ => FontSize::Medium,
            },
            _ => FontSize::Medium,
        }
    }

    pub fn from_str(value: &str) -> Result<Self, Error> {
        match value {
            "medium" => Ok(Self::Medium),
            "large" | "x-large" => Ok(Self::XLarge),
            "xx-large" => Ok(Self::XXLarge),
            _ => Err(Error::UnexpectedInput(format!(
                "font-size {:?} is not supported yet",
                value
            ))),
        }
    }

    pub fn from_px(value: f64) -> Self {
        if value >= 32.0 {
            Self::XXLarge
        } else if value >= 24.0 {
            Self::XLarge
        } else {
            Self::Medium
        }
    }

    pub fn px(&self) -> i64 {
        match self {
            FontSize::Medium => 16,
            FontSize::XLarge => 24,
            FontSize::XXLarge => 32,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum PositionType {
    Static,
    Relative,
    Absolute,
}

impl PositionType {
    pub fn from_str(value: &str) -> Result<Self, Error> {
        match value {
            "static" => Ok(Self::Static),
            "relative" => Ok(Self::Relative),
            "absolute" => Ok(Self::Absolute),
            _ => Err(Error::UnexpectedInput(format!(
                "position {:?} is not supported yet",
                value
            ))),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum DisplayType {
    Block,
    Inline,
    DisplayNone,
}

impl DisplayType {
    fn default(node: &Rc<RefCell<Node>>) -> Self {
        match &node.borrow().kind() {
            NodeKind::Document => DisplayType::Block,
            NodeKind::Element(e) => {
                if e.is_block_element() {
                    DisplayType::Block
                } else {
                    DisplayType::Inline
                }
            }
            NodeKind::Text(_) => DisplayType::Inline,
        }
    }

    pub fn from_str(s: &str) -> Result<Self, Error> {
        match s {
            "block" => Ok(Self::Block),
            "inline" => Ok(Self::Inline),
            "none" => Ok(Self::DisplayNone),
            _ => Err(Error::UnexpectedInput(format!(
                "display {:?} is not supported yet",
                s
            ))),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum TextDecoration {
    None,
    Underline,
}

impl TextDecoration {
    fn default(node: &Rc<RefCell<Node>>) -> Self {
        match &node.borrow().kind() {
            NodeKind::Element(element) => match element.kind() {
                ElementKind::A => TextDecoration::Underline,
                _ => TextDecoration::None,
            },
            _ => TextDecoration::None,
        }
    }

    pub fn from_str(value: &str) -> Result<Self, Error> {
        match value {
            "none" => Ok(Self::None),
            "underline" => Ok(Self::Underline),
            _ => Err(Error::UnexpectedInput(format!(
                "text-decoration {:?} is not supported yet",
                value
            ))),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

fn get_element_attribute(node: &Rc<RefCell<Node>>, name: &str) -> Option<String> {
    match node.borrow().kind() {
        NodeKind::Element(ref element) => element.get_attribute(name),
        _ => None,
    }
}

fn parse_html_color(value: &str) -> Option<Color> {
    let trimmed = value.trim();
    if trimmed.starts_with('#') {
        Color::from_code(trimmed).ok()
    } else {
        Color::from_name(trimmed).ok().or_else(|| {
            // Try as bare hex code (e.g., "d2b48c" without #).
            if trimmed.len() == 6 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
                Color::from_code(&format!("#{}", trimmed)).ok()
            } else {
                None
            }
        })
    }
}
