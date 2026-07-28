/// Minimal browser UI chrome: URL bar + back/forward buttons.
use tiny_skia::{Paint, Pixmap, Rect, Transform};

use crate::text_render::TextRenderer;

/// Height of the chrome bar in pixels.
pub const CHROME_HEIGHT: i64 = 44;

const CHROME_BG: (u8, u8, u8) = (0xF0, 0xF0, 0xF0);
const CHROME_BORDER: (u8, u8, u8) = (0xCC, 0xCC, 0xCC);
const BUTTON_TEXT_COLOR: (u8, u8, u8) = (0x33, 0x33, 0x33);
const URL_BAR_BG: (u8, u8, u8) = (0xFF, 0xFF, 0xFF);
const URL_BAR_BORDER: (u8, u8, u8) = (0xAA, 0xAA, 0xAA);

const BACK_BTN_X: i64 = 8;
const FORWARD_BTN_X: i64 = 36;
const RELOAD_BTN_X: i64 = 64;
const URL_BAR_X: i64 = 96;
const URL_BAR_Y: i64 = 8;
const URL_BAR_HEIGHT: i64 = 28;
const BUTTON_Y: i64 = 10;
const BUTTON_SIZE: i64 = 24;

pub struct ChromeState {
    pub url_text: String,
    pub cursor_pos: usize,
    pub is_focused: bool,
    /// Whether the session has a page to go back to.
    pub can_back: bool,
    /// Whether the session has a page to go forward to.
    pub can_forward: bool,
}

impl ChromeState {
    pub fn new() -> Self {
        Self {
            url_text: String::new(),
            cursor_pos: 0,
            is_focused: false,
            can_back: false,
            can_forward: false,
        }
    }

    pub fn set_url(&mut self, url: &str) {
        self.url_text = url.to_string();
        self.cursor_pos = self.url_text.len();
    }

    pub fn insert_char(&mut self, ch: char) {
        self.url_text.insert(self.cursor_pos, ch);
        self.cursor_pos += ch.len_utf8();
    }

    pub fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            let prev = self.url_text[..self.cursor_pos]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.url_text.drain(prev..self.cursor_pos);
            self.cursor_pos = prev;
        }
    }

    pub fn delete_forward(&mut self) {
        if self.cursor_pos < self.url_text.len() {
            let next = self.url_text[self.cursor_pos..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cursor_pos + i)
                .unwrap_or(self.url_text.len());
            self.url_text.drain(self.cursor_pos..next);
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor_pos > 0 {
            self.cursor_pos = self.url_text[..self.cursor_pos]
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .unwrap_or(0);
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor_pos < self.url_text.len() {
            self.cursor_pos = self.url_text[self.cursor_pos..]
                .char_indices()
                .nth(1)
                .map(|(i, _)| self.cursor_pos + i)
                .unwrap_or(self.url_text.len());
        }
    }

    pub fn select_all(&mut self) {
        self.cursor_pos = self.url_text.len();
    }

    pub fn get_url(&self) -> &str {
        &self.url_text
    }
}

pub enum ChromeAction {
    None,
    #[allow(dead_code)]
    Navigate(String),
    Back,
    Forward,
    Reload,
    FocusUrlBar,
}

/// Draw the chrome bar and return chrome-area hit actions.
pub fn draw_chrome(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    chrome: &ChromeState,
    viewport_width: u32,
) {
    let pw = viewport_width as f32;

    // Background.
    if let Some(rect) = Rect::from_xywh(0.0, 0.0, pw, CHROME_HEIGHT as f32) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(CHROME_BG.0, CHROME_BG.1, CHROME_BG.2, 255);
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }

    // Bottom border.
    if let Some(rect) = Rect::from_xywh(0.0, CHROME_HEIGHT as f32 - 1.0, pw, 1.0) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(CHROME_BORDER.0, CHROME_BORDER.1, CHROME_BORDER.2, 255);
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }

    // Back button.
    draw_button(
        pixmap,
        text_renderer,
        BACK_BTN_X,
        BUTTON_Y,
        "\u{25C0}",
        chrome.can_back,
    );
    // Forward button.
    draw_button(
        pixmap,
        text_renderer,
        FORWARD_BTN_X,
        BUTTON_Y,
        "\u{25B6}",
        chrome.can_forward,
    );
    // Reload button.
    draw_button(
        pixmap,
        text_renderer,
        RELOAD_BTN_X,
        BUTTON_Y,
        "\u{21BB}",
        true,
    );

    // URL bar background.
    let url_bar_width = (viewport_width as i64 - URL_BAR_X - 12).max(100);
    if let Some(rect) = Rect::from_xywh(
        URL_BAR_X as f32,
        URL_BAR_Y as f32,
        url_bar_width as f32,
        URL_BAR_HEIGHT as f32,
    ) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(URL_BAR_BG.0, URL_BAR_BG.1, URL_BAR_BG.2, 255);
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }

    // URL bar border.
    let border_color = if chrome.is_focused {
        (0x44, 0x88, 0xDD)
    } else {
        URL_BAR_BORDER
    };
    draw_rect_border(
        pixmap,
        URL_BAR_X,
        URL_BAR_Y,
        url_bar_width,
        URL_BAR_HEIGHT,
        border_color,
    );

    // URL text.
    let text_y = URL_BAR_Y + 20;
    text_renderer.draw_text(
        pixmap,
        &chrome.url_text,
        URL_BAR_X + 6,
        text_y,
        14,
        0x22,
        0x22,
        0x22,
        255,
        0,
        false,
    );

    // Cursor.
    if chrome.is_focused {
        let cursor_x = if chrome.cursor_pos == 0 {
            URL_BAR_X + 6
        } else {
            let prefix = &chrome.url_text[..chrome.cursor_pos];
            URL_BAR_X + 6 + text_renderer.measure_text(prefix, 14)
        };
        let pw = pixmap.width() as i64;
        let ph = pixmap.height() as i64;
        let data = pixmap.data_mut();
        for row in (URL_BAR_Y + 4)..(URL_BAR_Y + URL_BAR_HEIGHT - 4) {
            if row < 0 || row >= ph {
                continue;
            }
            if cursor_x >= 0 && cursor_x < pw {
                let idx = (row * pw + cursor_x) as usize * 4;
                if idx + 3 < data.len() {
                    data[idx] = 0x22;
                    data[idx + 1] = 0x22;
                    data[idx + 2] = 0x22;
                    data[idx + 3] = 255;
                }
            }
        }
    }
}

/// Handle a mouse click in the chrome area. Returns the action to perform.
pub fn handle_chrome_click(
    x: i64,
    y: i64,
    viewport_width: u32,
    chrome: &ChromeState,
) -> ChromeAction {
    if y < 0 || y >= CHROME_HEIGHT {
        return ChromeAction::None;
    }

    // Back button — only fire when history allows it.
    if x >= BACK_BTN_X
        && x < BACK_BTN_X + BUTTON_SIZE
        && y >= BUTTON_Y
        && y < BUTTON_Y + BUTTON_SIZE
    {
        return if chrome.can_back {
            ChromeAction::Back
        } else {
            ChromeAction::None
        };
    }
    // Forward button — only fire when history allows it.
    if x >= FORWARD_BTN_X
        && x < FORWARD_BTN_X + BUTTON_SIZE
        && y >= BUTTON_Y
        && y < BUTTON_Y + BUTTON_SIZE
    {
        return if chrome.can_forward {
            ChromeAction::Forward
        } else {
            ChromeAction::None
        };
    }
    // Reload button.
    if x >= RELOAD_BTN_X
        && x < RELOAD_BTN_X + BUTTON_SIZE
        && y >= BUTTON_Y
        && y < BUTTON_Y + BUTTON_SIZE
    {
        return ChromeAction::Reload;
    }
    // URL bar.
    let url_bar_width = (viewport_width as i64 - URL_BAR_X - 12).max(100);
    if x >= URL_BAR_X
        && x < URL_BAR_X + url_bar_width
        && y >= URL_BAR_Y
        && y < URL_BAR_Y + URL_BAR_HEIGHT
    {
        return ChromeAction::FocusUrlBar;
    }

    ChromeAction::None
}

fn draw_button(
    pixmap: &mut Pixmap,
    text_renderer: &mut TextRenderer,
    x: i64,
    y: i64,
    label: &str,
    enabled: bool,
) {
    // Button background — lighter when disabled.
    let bg = if enabled {
        (0xE0, 0xE0, 0xE0)
    } else {
        (0xF0, 0xF0, 0xF0)
    };
    if let Some(rect) = Rect::from_xywh(x as f32, y as f32, BUTTON_SIZE as f32, BUTTON_SIZE as f32)
    {
        let mut paint = Paint::default();
        paint.set_color_rgba8(bg.0, bg.1, bg.2, 255);
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }
    draw_rect_border(pixmap, x, y, BUTTON_SIZE, BUTTON_SIZE, (0xBB, 0xBB, 0xBB));

    // Button label — grayed out when disabled.
    let text_color = if enabled {
        BUTTON_TEXT_COLOR
    } else {
        (0xBB, 0xBB, 0xBB)
    };
    text_renderer.draw_text(
        pixmap,
        label,
        x + 4,
        y + 18,
        14,
        text_color.0,
        text_color.1,
        text_color.2,
        255,
        0,
        false,
    );
}

fn draw_rect_border(pixmap: &mut Pixmap, x: i64, y: i64, w: i64, h: i64, color: (u8, u8, u8)) {
    let mut paint = Paint::default();
    paint.set_color_rgba8(color.0, color.1, color.2, 255);

    // top
    if let Some(r) = Rect::from_xywh(x as f32, y as f32, w as f32, 1.0) {
        pixmap.fill_rect(r, &paint, Transform::identity(), None);
    }
    // bottom
    if let Some(r) = Rect::from_xywh(x as f32, (y + h - 1) as f32, w as f32, 1.0) {
        pixmap.fill_rect(r, &paint, Transform::identity(), None);
    }
    // left
    if let Some(r) = Rect::from_xywh(x as f32, y as f32, 1.0, h as f32) {
        pixmap.fill_rect(r, &paint, Transform::identity(), None);
    }
    // right
    if let Some(r) = Rect::from_xywh((x + w - 1) as f32, y as f32, 1.0, h as f32) {
        pixmap.fill_rect(r, &paint, Transform::identity(), None);
    }
}

/// Width of the scrollbar in pixels.
pub const SCROLLBAR_WIDTH: i64 = 12;

/// Minimum thumb height in pixels.
const SCROLLBAR_THUMB_MIN: i64 = 20;

/// Draw a vertical scrollbar on the right edge of the page area.
///
/// - `scroll_y`: current scroll offset in CSS pixels.
/// - `content_height`: total height of the page content in CSS pixels.
/// - `viewport_width` / `viewport_height`: window dimensions in physical pixels.
///
/// The scrollbar is only drawn when the content is taller than the visible
/// page area (`content_height > page_height`).
pub fn draw_scrollbar(
    pixmap: &mut Pixmap,
    scroll_y: i64,
    content_height: i64,
    viewport_width: u32,
    viewport_height: u32,
) {
    let page_height = viewport_height as i64 - CHROME_HEIGHT;
    if page_height <= 0 || content_height <= page_height {
        return;
    }

    let track_x = viewport_width as i64 - SCROLLBAR_WIDTH;
    let track_y = CHROME_HEIGHT;

    // Track background.
    if let Some(rect) = Rect::from_xywh(
        track_x as f32,
        track_y as f32,
        SCROLLBAR_WIDTH as f32,
        page_height as f32,
    ) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(0xE8, 0xE8, 0xE8, 220);
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }

    // Thumb sizing.
    // Spec: CSS OM View §11 — scrollbar thumb proportional to scroll range.
    let thumb_ratio = (page_height as f64 / content_height as f64).min(1.0);
    let thumb_height = ((page_height as f64 * thumb_ratio) as i64).max(SCROLLBAR_THUMB_MIN);
    let scroll_range = content_height - page_height;
    let thumb_travel = (page_height - thumb_height).max(0);
    let thumb_y = track_y
        + if scroll_range > 0 {
            (thumb_travel as f64 * scroll_y as f64 / scroll_range as f64) as i64
        } else {
            0
        };

    // Thumb.
    if let Some(rect) = Rect::from_xywh(
        (track_x + 2) as f32,
        thumb_y as f32,
        (SCROLLBAR_WIDTH - 4) as f32,
        thumb_height as f32,
    ) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(0xA0, 0xA0, 0xA0, 200);
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }
}
