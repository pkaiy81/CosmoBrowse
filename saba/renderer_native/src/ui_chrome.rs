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
}

impl ChromeState {
    pub fn new() -> Self {
        Self {
            url_text: String::new(),
            cursor_pos: 0,
            is_focused: false,
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
    draw_button(pixmap, text_renderer, BACK_BTN_X, BUTTON_Y, "\u{25C0}");
    // Forward button.
    draw_button(pixmap, text_renderer, FORWARD_BTN_X, BUTTON_Y, "\u{25B6}");
    // Reload button.
    draw_button(pixmap, text_renderer, RELOAD_BTN_X, BUTTON_Y, "\u{21BB}");

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
pub fn handle_chrome_click(x: i64, y: i64, viewport_width: u32) -> ChromeAction {
    if y < 0 || y >= CHROME_HEIGHT {
        return ChromeAction::None;
    }

    // Back button.
    if x >= BACK_BTN_X && x < BACK_BTN_X + BUTTON_SIZE && y >= BUTTON_Y && y < BUTTON_Y + BUTTON_SIZE {
        return ChromeAction::Back;
    }
    // Forward button.
    if x >= FORWARD_BTN_X && x < FORWARD_BTN_X + BUTTON_SIZE && y >= BUTTON_Y && y < BUTTON_Y + BUTTON_SIZE {
        return ChromeAction::Forward;
    }
    // Reload button.
    if x >= RELOAD_BTN_X && x < RELOAD_BTN_X + BUTTON_SIZE && y >= BUTTON_Y && y < BUTTON_Y + BUTTON_SIZE {
        return ChromeAction::Reload;
    }
    // URL bar.
    let url_bar_width = (viewport_width as i64 - URL_BAR_X - 12).max(100);
    if x >= URL_BAR_X && x < URL_BAR_X + url_bar_width && y >= URL_BAR_Y && y < URL_BAR_Y + URL_BAR_HEIGHT {
        return ChromeAction::FocusUrlBar;
    }

    ChromeAction::None
}

fn draw_button(pixmap: &mut Pixmap, text_renderer: &mut TextRenderer, x: i64, y: i64, label: &str) {
    // Button background.
    if let Some(rect) = Rect::from_xywh(x as f32, y as f32, BUTTON_SIZE as f32, BUTTON_SIZE as f32) {
        let mut paint = Paint::default();
        paint.set_color_rgba8(0xE0, 0xE0, 0xE0, 255);
        pixmap.fill_rect(rect, &paint, Transform::identity(), None);
    }
    draw_rect_border(pixmap, x, y, BUTTON_SIZE, BUTTON_SIZE, (0xBB, 0xBB, 0xBB));

    // Button label.
    text_renderer.draw_text(
        pixmap,
        label,
        x + 4,
        y + 18,
        14,
        BUTTON_TEXT_COLOR.0,
        BUTTON_TEXT_COLOR.1,
        BUTTON_TEXT_COLOR.2,
        255,
        0,
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
