// renderer-wasm/src/lib.rs

// 1. wasm-bindgen と serde をインポート
use wasm_bindgen::prelude::*;
use serde::Serialize;
use wasm_bindgen::JsValue;
use serde_json;

// 2. 既存のエンジン（saba_core）を取り込む
use saba_core::renderer::html::parser::HtmlParser;
use saba_core::renderer::html::token::HtmlTokenizer;
use saba_core::renderer::dom::api::get_style_content;
use saba_core::renderer::css::cssom::CssParser;
use saba_core::renderer::css::token::CssTokenizer;
use saba_core::renderer::layout::layout_view::LayoutView;
use saba_core::display_item::DisplayItem;
use saba_core::renderer::layout::computed_style::FontSize;

// 3. JS 側にシリアライズして渡すための構造体／enum 定義
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum DrawCommand {
    Rect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        color: String,
    },
    Text {
        x: f64,
        y: f64,
        text: String,
        font: String,
        size: f64,
        color: String,
    },
    // 他にも必要なら e.g. Line, Image などを追加
}

// 4. wasm-bindgen でエクスポートする関数
#[wasm_bindgen]
pub fn parse_and_render(html: &str, _canvas_width: f64, _canvas_height: f64) -> JsValue {
    // --- HTML → DOM ---
    let tokenizer = HtmlTokenizer::new(html.to_string());
    let mut parser = HtmlParser::new(tokenizer);
    let window = parser.construct_tree();
    let dom = window.borrow().document();

    // --- CSSOM の生成 ---
    let css_text = get_style_content(dom.clone());
    let mut css_parser = CssParser::new(CssTokenizer::new(css_text));
    let stylesheet = css_parser.parse_stylesheet();

    // --- レイアウト & ペイント ---
    let layout_view = LayoutView::new(dom.clone(), &stylesheet);
    let items: Vec<DisplayItem> = layout_view.paint();

    // --- DisplayItem → DrawCommand に変換 ---
    let commands: Vec<DrawCommand> = items
        .into_iter()
        .map(|item| match item {
            DisplayItem::Rect {
                style,
                layout_point,
                layout_size,
            } => {
                DrawCommand::Rect {
                    x: layout_point.x() as f64,
                    y: layout_point.y() as f64,
                    width: layout_size.width() as f64,
                    height: layout_size.height() as f64,
                    // 公開メソッド `.code()` で取得して String に変換
                    color: style.background_color().code().to_string(),
                }
            }
            DisplayItem::Text {
                text,
                style,
                layout_point,
            } => {
                // フォントサイズをピクセルにマッピング
                let size_px = match style.font_size() {
                    FontSize::Medium => 16.0,
                    FontSize::XLarge => 24.0,
                    FontSize::XXLarge => 32.0,
                };
                DrawCommand::Text {
                    x: layout_point.x() as f64,
                    y: layout_point.y() as f64,
                    text,
                    font: "sans-serif".to_string(),
                    size: size_px,
                    color: style.color().code().to_string(),
                }
            }
        })
        .collect();

    // 5. JS に渡すため JSON シリアライズ
    let json = serde_json::to_string(&commands)
        .expect("failed to serialize draw commands");
    JsValue::from_str(&json)
}
