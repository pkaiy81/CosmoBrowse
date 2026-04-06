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
use painter::{render_commands, ImageCache};
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
    image_cache: ImageCache,
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
    save_screenshot: bool,
}

impl App {
    fn new() -> Self {
        Self {
            window_state: None,
            text_renderer: TextRenderer::new(),
            image_cache: ImageCache::new(),
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
            save_screenshot: false,
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
                // Sync the layout engine's viewport with the current window size.
                // The session snapshot may carry a stale viewport from a previous run.
                let _ = self.bridge.set_viewport(self.viewport_width, self.viewport_height);
                self.chrome.set_url(&self.bridge.current_url());
                // Spec: HTML Living Standard §7.4 — scroll to fragment anchor if present.
                // https://html.spec.whatwg.org/multipage/browsing-the-web.html#scroll-to-fragid
                self.scroll_y = self.bridge.anchor_scroll_y();
                self.update_nav_state();
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
        self.request_redraw();
    }

    /// Refresh back/forward button states from the navigation history.
    fn update_nav_state(&mut self) {
        self.chrome.can_back = self.bridge.can_go_back();
        self.chrome.can_forward = self.bridge.can_go_forward();
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
        ui_chrome::draw_chrome(&mut pixmap, &mut self.text_renderer, &self.chrome, width);

        // Draw page content.
        let mut all_hit_regions = Vec::new();
        let base_url = self.bridge.current_url();
        let frame_commands = self.bridge.collect_paint_commands();
        for (frame_id, commands) in &frame_commands {
            let regions = render_commands(
                &mut pixmap,
                commands,
                &mut self.text_renderer,
                &mut self.image_cache,
                &base_url,
                self.scroll_y,
                CHROME_HEIGHT,
                frame_id,
            );
            all_hit_regions.extend(regions);
        }
        self.hit_regions = all_hit_regions;

        // Draw scrollbar over the page area.
        let content_height = self.bridge.content_height();
        ui_chrome::draw_scrollbar(&mut pixmap, self.scroll_y, content_height, width, height);

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

        // Save a PNG snapshot if requested (e.g. via Ctrl+S).
        if self.save_screenshot {
            self.save_screenshot = false;
            let path = "/tmp/cosmo_screenshot.png";
            match pixmap.save_png(path) {
                Ok(()) => eprintln!("[SCREENSHOT] Saved to {}", path),
                Err(e) => eprintln!("[SCREENSHOT] Failed: {}", e),
            }
        }
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
                // Ctrl+S: save screenshot to /tmp/cosmo_screenshot.png.
                if ch.eq("s") && !event.repeat && is_ctrl_pressed(&event) {
                    self.save_screenshot = true;
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
            let action = ui_chrome::handle_chrome_click(x, y, self.viewport_width, &self.chrome);
            match action {
                ChromeAction::Back => {
                    if self.bridge.back().is_ok() {
                        self.chrome.set_url(&self.bridge.current_url());
                        self.update_nav_state();
                        self.scroll_y = 0;
                    }
                    self.needs_redraw = true;
                }
                ChromeAction::Forward => {
                    if self.bridge.forward().is_ok() {
                        self.chrome.set_url(&self.bridge.current_url());
                        self.update_nav_state();
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
            // Save the current top-level URL so we can detect root navigations.
            let prev_url = self.bridge.current_url();
            match self
                .bridge
                .activate_link(&frame_id, &href, target.as_deref())
            {
                Ok(()) => {
                    self.chrome.set_url(&self.bridge.current_url());
                    // Only reset scroll when the root-level URL changes (i.e. a new
                    // standalone page was loaded).  For child-frame-only navigations
                    // (e.g. frameset target="right" or in-page anchor in a sub-frame)
                    // the top-level URL stays the same and we preserve the current
                    // scroll position so the viewport does not jump to the top.
                    // Spec: HTML Living Standard §7.4 — navigating to a fragment.
                    // https://html.spec.whatwg.org/multipage/browsing-the-web.html#navigate
                    if self.bridge.current_url() != prev_url {
                        self.scroll_y = self.bridge.anchor_scroll_y();
                    }
                    self.update_nav_state();
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
        let page_height = self.viewport_height as i64 - CHROME_HEIGHT;
        let content_height = self.bridge.content_height();
        let max_scroll = (content_height - page_height).max(0);
        self.scroll_y = (self.scroll_y - delta_y as i64 * 40).max(0).min(max_scroll);
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
                // Notify the layout engine so it can reflow content against
                // the new viewport width.
                // Spec: CSS2.2 §10.1 — viewport is the containing block for
                // the initial block formatting context.
                // https://www.w3.org/TR/CSS22/visudet.html#containing-block-details
                if let Err(e) = self.bridge.set_viewport(size.width, size.height) {
                    self.status_message = format!("Viewport error: {}", e);
                }
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

/// Headless screenshot mode: render `url` to a PNG at `out_path` without opening a window.
fn headless_screenshot(url: &str, out_path: &str) {
    let url = if url.contains("://") {
        url.to_string()
    } else {
        format!("https://{}", url)
    };

    let width = DEFAULT_WIDTH;
    let height = DEFAULT_HEIGHT;

    let mut text_renderer = TextRenderer::new();
    let mut image_cache = ImageCache::new();
    let mut bridge = AppBridge::new();

    if let Err(e) = bridge.navigate(&url) {
        eprintln!("[SCREENSHOT] navigate error: {}", e);
        return;
    }
    // set_viewport must be called after navigate (requires a loaded page)
    // to override any stale viewport from the session snapshot.
    if let Err(e) = bridge.set_viewport(width, height) {
        eprintln!("[SCREENSHOT] set_viewport error: {}", e);
        return;
    }

    let mut pixmap = tiny_skia::Pixmap::new(width, height).expect("Failed to create pixmap");
    pixmap.fill(tiny_skia::Color::WHITE);

    let chrome = ChromeState::new();
    ui_chrome::draw_chrome(&mut pixmap, &mut text_renderer, &chrome, width);

    let base_url = bridge.current_url();
    let frame_commands = bridge.collect_paint_commands();
    for (frame_id, commands) in &frame_commands {
        render_commands(
            &mut pixmap,
            commands,
            &mut text_renderer,
            &mut image_cache,
            &base_url,
            0,
            CHROME_HEIGHT,
            frame_id,
        );
    }

    let content_height = bridge.content_height();
    ui_chrome::draw_scrollbar(&mut pixmap, 0, content_height, width, height);

    match pixmap.save_png(out_path) {
        Ok(()) => eprintln!("[SCREENSHOT] Saved to {}", out_path),
        Err(e) => eprintln!("[SCREENSHOT] Failed: {}", e),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // --screenshot <url> [out.png]
    if args.get(1).map(|s| s.as_str()) == Some("--screenshot") {
        let url = args.get(2).map(|s| s.as_str()).unwrap_or("about:blank");
        let out = args.get(3).map(|s| s.as_str()).unwrap_or("/tmp/cosmo_screenshot.png");
        headless_screenshot(url, out);
        return;
    }

    let url = args.get(1).cloned();

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
