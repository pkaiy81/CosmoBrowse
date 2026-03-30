mod app_bridge;
mod color;
mod hit_test;
mod painter;
mod text_render;
mod ui_chrome;

use std::num::NonZeroU32;
use std::rc::Rc;

use app_bridge::AppBridge;
use hit_test::hit_test;
use painter::render_commands;
use text_render::TextRenderer;
use ui_chrome::{ChromeAction, ChromeState, CHROME_HEIGHT};
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition},
    event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent},
    event_loop::{ActiveEventLoop, EventLoop},
    keyboard::{Key, NamedKey},
    window::{CursorIcon, WindowAttributes, WindowId},
};

const DEFAULT_WIDTH: u32 = 1024;
const DEFAULT_HEIGHT: u32 = 768;

/// Holds the winit Window and softbuffer Surface together, sharing the
/// same `Rc<winit::window::Window>`.
struct WindowState {
    window: Rc<winit::window::Window>,
    surface: softbuffer::Surface<Rc<winit::window::Window>, Rc<winit::window::Window>>,
}

struct App {
    window_state: Option<WindowState>,
    text_renderer: TextRenderer,
    bridge: AppBridge,
    chrome: ChromeState,
    scroll_y: i64,
    hit_regions: Vec<hit_test::HitRegion>,
    mouse_pos: (i64, i64),
    viewport_width: u32,
    viewport_height: u32,
    needs_redraw: bool,
    status_message: String,
    pending_url: Option<String>,
}

impl App {
    fn new() -> Self {
        Self {
            window_state: None,
            text_renderer: TextRenderer::new(),
            bridge: AppBridge::new(),
            chrome: ChromeState::new(),
            scroll_y: 0,
            hit_regions: Vec::new(),
            mouse_pos: (0, 0),
            viewport_width: DEFAULT_WIDTH,
            viewport_height: DEFAULT_HEIGHT,
            needs_redraw: true,
            status_message: String::new(),
            pending_url: None,
        }
    }

    fn request_redraw(&self) {
        if let Some(ws) = &self.window_state {
            ws.window.request_redraw();
        }
    }

    fn navigate(&mut self, url: &str) {
        self.status_message = format!("Loading {}...", url);
        self.needs_redraw = true;
        self.redraw();

        match self.bridge.navigate(url) {
            Ok(()) => {
                self.chrome.set_url(&self.bridge.current_url());
                self.scroll_y = 0;
                self.status_message.clear();
                if let Some(ws) = &self.window_state {
                    let title = self.bridge.current_title();
                    let display_title = if title.is_empty() {
                        "CosmoBrowse".to_string()
                    } else {
                        format!("{} - CosmoBrowse", title)
                    };
                    ws.window.set_title(&display_title);
                }
            }
            Err(e) => {
                self.status_message = format!("Error: {}", e);
            }
        }
        self.needs_redraw = true;
    }

    fn redraw(&mut self) {
        let Some(ws) = &mut self.window_state else {
            return;
        };

        let width = self.viewport_width;
        let height = self.viewport_height;

        if width == 0 || height == 0 {
            return;
        }

        let mut pixmap = match tiny_skia::Pixmap::new(width, height) {
            Some(p) => p,
            None => return,
        };

        // Fill background white.
        pixmap.fill(tiny_skia::Color::WHITE);

        // Draw chrome.
        ui_chrome::draw_chrome(
            &mut pixmap,
            &mut self.text_renderer,
            &self.chrome,
            width,
        );

        // Draw page content.
        let mut all_hit_regions = Vec::new();
        let frame_commands = self.bridge.collect_paint_commands();
        for (frame_id, commands) in &frame_commands {
            let regions = render_commands(
                &mut pixmap,
                commands,
                &mut self.text_renderer,
                self.scroll_y,
                CHROME_HEIGHT,
                frame_id,
            );
            all_hit_regions.extend(regions);
        }
        self.hit_regions = all_hit_regions;

        // Draw status message.
        if !self.status_message.is_empty() {
            let status_y = height as i64 - 4;
            self.text_renderer.draw_text(
                &mut pixmap,
                &self.status_message,
                8,
                status_y,
                12,
                0x66,
                0x66,
                0x66,
                200,
                0,
            );
        }

        // Present to window.
        ws.surface
            .resize(
                NonZeroU32::new(width).unwrap(),
                NonZeroU32::new(height).unwrap(),
            )
            .expect("Failed to resize surface");

        let mut buffer = ws.surface.buffer_mut().expect("Failed to get buffer");
        let data = pixmap.data();
        for i in 0..(width * height) as usize {
            let idx = i * 4;
            let r = data[idx] as u32;
            let g = data[idx + 1] as u32;
            let b = data[idx + 2] as u32;
            buffer[i] = (r << 16) | (g << 8) | b;
        }
        buffer.present().expect("Failed to present buffer");
        self.needs_redraw = false;
    }

    fn handle_key(&mut self, event: KeyEvent) {
        if event.state != ElementState::Pressed {
            return;
        }

        match event.logical_key.as_ref() {
            Key::Named(NamedKey::Enter) => {
                if self.chrome.is_focused {
                    let url = self.chrome.get_url().to_string();
                    self.chrome.is_focused = false;
                    if !url.is_empty() {
                        let url = if url.contains("://") {
                            url
                        } else {
                            format!("https://{}", url)
                        };
                        self.navigate(&url);
                    }
                }
            }
            Key::Named(NamedKey::Backspace) => {
                if self.chrome.is_focused {
                    self.chrome.backspace();
                    self.needs_redraw = true;
                }
            }
            Key::Named(NamedKey::Delete) => {
                if self.chrome.is_focused {
                    self.chrome.delete_forward();
                    self.needs_redraw = true;
                }
            }
            Key::Named(NamedKey::ArrowLeft) => {
                if self.chrome.is_focused {
                    self.chrome.move_left();
                    self.needs_redraw = true;
                }
            }
            Key::Named(NamedKey::ArrowRight) => {
                if self.chrome.is_focused {
                    self.chrome.move_right();
                    self.needs_redraw = true;
                }
            }
            Key::Named(NamedKey::Escape) => {
                self.chrome.is_focused = false;
                self.needs_redraw = true;
            }
            Key::Character(ch) => {
                // Ctrl+L: focus URL bar.
                if ch.eq("l") && !event.repeat && is_ctrl_pressed(&event) {
                    self.chrome.is_focused = true;
                    self.chrome.select_all();
                    self.needs_redraw = true;
                    return;
                }

                if self.chrome.is_focused {
                    for c in ch.chars() {
                        if !c.is_control() {
                            self.chrome.insert_char(c);
                        }
                    }
                    self.needs_redraw = true;
                }
            }
            _ => {}
        }
    }

    fn handle_mouse_click(&mut self, x: i64, y: i64) {
        // Chrome area click.
        if y < CHROME_HEIGHT {
            let action = ui_chrome::handle_chrome_click(x, y, self.viewport_width);
            match action {
                ChromeAction::Back => {
                    if self.bridge.back().is_ok() {
                        self.chrome.set_url(&self.bridge.current_url());
                        self.scroll_y = 0;
                    }
                    self.needs_redraw = true;
                }
                ChromeAction::Forward => {
                    if self.bridge.forward().is_ok() {
                        self.chrome.set_url(&self.bridge.current_url());
                        self.scroll_y = 0;
                    }
                    self.needs_redraw = true;
                }
                ChromeAction::Reload => {
                    if self.bridge.reload().is_ok() {
                        self.chrome.set_url(&self.bridge.current_url());
                    }
                    self.needs_redraw = true;
                }
                ChromeAction::FocusUrlBar => {
                    self.chrome.is_focused = true;
                    self.chrome.select_all();
                    self.needs_redraw = true;
                }
                ChromeAction::Navigate(url) => {
                    self.navigate(&url);
                }
                ChromeAction::None => {}
            }
            return;
        }

        // Page area: hit-test links.
        if let Some(region) = hit_test(&self.hit_regions, x, y) {
            let href = region.href.clone();
            let target = region.target.clone();
            let frame_id = region.frame_id.clone();
            match self
                .bridge
                .activate_link(&frame_id, &href, target.as_deref())
            {
                Ok(()) => {
                    self.chrome.set_url(&self.bridge.current_url());
                    self.scroll_y = 0;
                }
                Err(e) => {
                    self.status_message = format!("Error: {}", e);
                }
            }
            self.needs_redraw = true;
        } else {
            // Unfocus URL bar if clicking on page area.
            if self.chrome.is_focused {
                self.chrome.is_focused = false;
                self.needs_redraw = true;
            }
        }
    }

    fn handle_scroll(&mut self, delta_y: f64) {
        self.scroll_y = (self.scroll_y - delta_y as i64 * 40).max(0);
        self.needs_redraw = true;
    }

    fn update_cursor(&self) {
        let Some(ws) = &self.window_state else {
            return;
        };
        let (mx, my) = self.mouse_pos;

        if my < CHROME_HEIGHT {
            ws.window.set_cursor(CursorIcon::Default);
            return;
        }

        if hit_test(&self.hit_regions, mx, my).is_some() {
            ws.window.set_cursor(CursorIcon::Pointer);
        } else {
            ws.window.set_cursor(CursorIcon::Default);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window_state.is_some() {
            return;
        }

        let attrs = WindowAttributes::default()
            .with_title("CosmoBrowse")
            .with_inner_size(LogicalSize::new(DEFAULT_WIDTH, DEFAULT_HEIGHT));
        let window = Rc::new(
            event_loop
                .create_window(attrs)
                .expect("Failed to create window"),
        );
        let context =
            softbuffer::Context::new(window.clone()).expect("Failed to create softbuffer context");
        let surface =
            softbuffer::Surface::new(&context, window.clone()).expect("Failed to create surface");

        self.window_state = Some(WindowState { window, surface });

        // Navigate to the pending URL if one was provided on the command line.
        if let Some(url) = self.pending_url.take() {
            self.navigate(&url);
        }
        self.request_redraw();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                self.viewport_width = size.width;
                self.viewport_height = size.height;
                self.needs_redraw = true;
                self.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                self.redraw();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.handle_key(event);
                if self.needs_redraw {
                    self.request_redraw();
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let (x, y) = self.mouse_pos;
                self.handle_mouse_click(x, y);
                if self.needs_redraw {
                    self.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse_pos = (position.x as i64, position.y as i64);
                self.update_cursor();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y as f64,
                    MouseScrollDelta::PixelDelta(PhysicalPosition { y, .. }) => y / 40.0,
                };
                self.handle_scroll(dy);
                self.request_redraw();
            }
            _ => {}
        }
    }
}

fn is_ctrl_pressed(event: &KeyEvent) -> bool {
    // winit 0.30: when Ctrl is held, `text` is typically None for letter keys.
    event.text.is_none()
}

fn main() {
    let url = std::env::args().nth(1);

    let event_loop = EventLoop::new().expect("Failed to create event loop");
    let mut app = App::new();

    if let Some(url) = &url {
        app.chrome.set_url(url);
        let url = if url.contains("://") {
            url.clone()
        } else {
            format!("https://{}", url)
        };
        app.pending_url = Some(url);
    }

    event_loop.run_app(&mut app).expect("Event loop failed");
}
