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
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
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

    pub fn bottom(&self) -> f64 {
        self.bottom
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComputedStyle {
    background_color: Option<Color>,
    color: Option<Color>,
    display: Option<DisplayType>,
    font_size: Option<FontSize>,
    text_decoration: Option<TextDecoration>,
    height: Option<f64>,
    height_ratio: Option<f64>,
    width: Option<f64>,
    width_ratio: Option<f64>,
    margin_horizontal_auto: bool,
    margin: Option<EdgeSize>,
    padding: Option<EdgeSize>,
}

impl ComputedStyle {
    pub fn new() -> Self {
        Self {
            background_color: None,
            color: None,
            display: None,
            font_size: None,
            text_decoration: None,
            height: None,
            height_ratio: None,
            width: None,
            width_ratio: None,
            margin_horizontal_auto: false,
            margin: None,
            padding: None,
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
            if self.font_size.is_none() && parent_style.font_size() != FontSize::Medium {
                self.font_size = Some(parent_style.font_size());
            }
            if self.text_decoration.is_none()
                && parent_style.text_decoration() != TextDecoration::None
            {
                self.text_decoration = Some(parent_style.text_decoration());
            }
        }

        if self.background_color.is_none() {
            self.background_color = Some(match node.borrow().element_kind() {
                Some(ElementKind::Button) | Some(ElementKind::Img) | Some(ElementKind::Input) => {
                    Color::lightgray()
                }
                _ => Color::white(),
            });
        }
        if self.color.is_none() {
            self.color = Some(Color::black());
        }
        if self.display.is_none() {
            self.display = Some(DisplayType::default(node));
        }
        if self.font_size.is_none() {
            self.font_size = Some(FontSize::default(node));
        }
        if self.text_decoration.is_none() {
            self.text_decoration = Some(TextDecoration::default(node));
        }
        if self.height.is_none() {
            self.height = Some(0.0);
        }
        if self.width.is_none() {
            self.width = Some(0.0);
        }
        if self.margin.is_none() {
            self.margin = Some(EdgeSize::zero());
        }
        if self.padding.is_none() {
            self.padding = Some(EdgeSize::zero());
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
        self.display.expect("failed to access CSS property: display")
    }

    pub fn set_font_size(&mut self, font_size: FontSize) {
        self.font_size = Some(font_size);
    }

    pub fn font_size(&self) -> FontSize {
        self.font_size
            .expect("failed to access CSS property: font-size")
    }

    pub fn set_text_decoration(&mut self, text_decoration: TextDecoration) {
        self.text_decoration = Some(text_decoration);
    }

    pub fn text_decoration(&self) -> TextDecoration {
        self.text_decoration
            .expect("failed to access CSS property: text-decoration")
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

    pub fn set_margin_horizontal_auto(&mut self, enabled: bool) {
        self.margin_horizontal_auto = enabled;
    }

    pub fn margin_horizontal_auto(&self) -> bool {
        self.margin_horizontal_auto
    }

    pub fn margin(&self) -> EdgeSize {
        self.margin.expect("failed to access CSS property: margin")
    }

    pub fn set_padding_all(&mut self, value: f64) {
        self.padding = Some(EdgeSize::all(value));
    }

    pub fn padding(&self) -> EdgeSize {
        self.padding.expect("failed to access CSS property: padding")
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

    pub fn black() -> Self {
        Self {
            name: Some("black".to_string()),
            code: "#000000".to_string(),
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







