use crate::browser::Browser;
use crate::display_item::DisplayItem;
use crate::http::HttpResponse;
use crate::renderer::css::cssom::CssParser;
use crate::renderer::css::cssom::StyleSheet;
use crate::renderer::css::token::CssTokenizer;
use crate::renderer::dom::api::get_js_content;
use crate::renderer::dom::api::get_style_content;
use crate::renderer::dom::api::get_stylesheet_links;
use crate::renderer::dom::api::get_title_content;
use crate::renderer::dom::node::ElementKind;
use crate::renderer::dom::node::NodeKind;
use crate::renderer::dom::node::Window;
use crate::renderer::html::parser::HtmlParser;
use crate::renderer::html::token::HtmlTokenizer;
use crate::renderer::js::ast::JsParser;
use crate::renderer::js::runtime::JsRuntime;
use crate::renderer::js::token::JsLexer;
use crate::renderer::layout::layout_object::LayoutSize;
use crate::renderer::layout::layout_view::LayoutView;
use alloc::rc::Rc;
use alloc::rc::Weak;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

#[derive(Debug, Clone)]
pub struct Page {
    browser: Weak<RefCell<Browser>>,
    frame: Option<Rc<RefCell<Window>>>,
    style: Option<StyleSheet>,
    layout_view: Option<LayoutView>,
    display_items: Vec<DisplayItem>,
}

impl Page {
    pub fn new() -> Self {
        Self {
            browser: Weak::new(),
            frame: None,
            style: None,
            layout_view: None,
            display_items: Vec::new(),
        }
    }

    pub fn set_browser(&mut self, browser: Weak<RefCell<Browser>>) {
        self.browser = browser;
    }

    pub fn receive_response(
        &mut self,
        response: HttpResponse,
        extra_style: String,
        viewport_width: i64,
    ) {
        self.create_frame(response.body(), extra_style);
        self.execute_js();
        self.set_layout_view(viewport_width);
        self.paint_tree();
    }

    pub fn reflow(&mut self, viewport_width: i64) {
        self.set_layout_view(viewport_width);
        self.paint_tree();
    }

    fn execute_js(&mut self) {
        let dom = match &self.frame {
            Some(frame) => frame.borrow().document(),
            None => return,
        };

        let js = get_js_content(dom.clone());
        let lexer = JsLexer::new(js);

        let mut parser = JsParser::new(lexer);
        let ast = parser.parse_ast();

        let mut runtime = JsRuntime::new(dom);
        runtime.execute(&ast);
    }

    fn create_frame(&mut self, html: String, extra_style: String) {
        let html_tokenizer = HtmlTokenizer::new(html);
        let frame = HtmlParser::new(html_tokenizer).construct_tree();
        let dom = frame.borrow().document();

        let mut style = get_style_content(dom.clone());
        if !extra_style.trim().is_empty() {
            if !style.is_empty() {
                style.push('\n');
            }
            style.push_str(&extra_style);
        }
        let css_tokenizer = CssTokenizer::new(style);
        let cssom = CssParser::new(css_tokenizer).parse_stylesheet();

        self.frame = Some(frame);
        self.style = Some(cssom);
    }

    fn set_layout_view(&mut self, viewport_width: i64) {
        let dom = match &self.frame {
            Some(frame) => frame.borrow().document(),
            None => return,
        };

        let style = match self.style.clone() {
            Some(style) => style,
            None => return,
        };

        let layout_view = LayoutView::new(dom, &style, viewport_width);
        self.layout_view = Some(layout_view);
    }

    fn paint_tree(&mut self) {
        if let Some(layout_view) = &self.layout_view {
            self.display_items = layout_view.paint();
        }
    }

    pub fn display_items(&self) -> Vec<DisplayItem> {
        self.display_items.clone()
    }

    pub fn stylesheet_links(&self) -> Vec<String> {
        self.frame
            .as_ref()
            .map(|frame| get_stylesheet_links(frame.borrow().document()))
            .unwrap_or_default()
    }

    pub fn title(&self) -> Option<String> {
        self.frame
            .as_ref()
            .and_then(|frame| get_title_content(frame.borrow().document()))
    }

    pub fn content_size(&self) -> LayoutSize {
        let mut width = 0;
        let mut height = 0;
        for item in &self.display_items {
            match item {
                DisplayItem::Rect {
                    layout_point,
                    layout_size,
                    ..
                } => {
                    width = width.max(layout_point.x() + layout_size.width());
                    height = height.max(layout_point.y() + layout_size.height());
                }
                DisplayItem::Text {
                    layout_point,
                    text,
                    style,
                    ..
                } => {
                    let text_width = text.len() as i64 * 8 * (style.font_size().px() / 16).max(1);
                    width = width.max(layout_point.x() + text_width);
                    height = height.max(layout_point.y() + style.font_size().px() + 4);
                }
            }
        }
        LayoutSize::new(width, height)
    }

    pub fn clear_display_items(&mut self) {
        self.display_items = Vec::new();
    }

    pub fn clicked(&self, position: (i64, i64)) -> Option<String> {
        let view = match &self.layout_view {
            Some(v) => v,
            None => return None,
        };

        if let Some(n) = view.find_node_by_position(position) {
            if let Some(parent) = n.borrow().parent().upgrade() {
                if let NodeKind::Element(e) = parent.borrow().node_kind() {
                    if e.kind() == ElementKind::A {
                        return e.get_attribute("href");
                    }
                }
            }
        }

        None
    }
}

