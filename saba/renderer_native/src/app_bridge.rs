/// Bridge between the native renderer and the browser engine (NativeAdapter).
use adapter_native::{BrowserFrameDto, BrowserPageDto, NativeAdapter};
use cosmo_engine::paint_commands::PaintCommand;
use cosmo_runtime::{scene_items_to_paint_commands, FetchWaker, FrameRect, LivePage, SceneItem};

/// A persistent script host for one content frame, kept alive so async work
/// (fetch/XHR/timers) can settle after first paint and re-layout. Each frame
/// (root and — for framesets — every child) gets its own; ScriptHost per-page
/// state now lives on the host (plan D5), so multiple can coexist on the
/// renderer thread (the active one is swapped in per call).
struct LiveFrame {
    frame_id: String,
    page: LivePage,
    rect: FrameRect,
}

pub struct AppBridge {
    adapter: NativeAdapter,
    current_page: Option<BrowserPageDto>,
    /// One script host per content frame (root + frameset children).
    live_frames: Vec<LiveFrame>,
    /// Fires when async work completes so the event loop wakes to pump.
    waker: Option<FetchWaker>,
}

impl AppBridge {
    pub fn new() -> Self {
        Self {
            adapter: NativeAdapter::default(),
            current_page: None,
            live_frames: Vec::new(),
            waker: None,
        }
    }

    /// Install the wake-up callback fired when a fetch/XHR response is ready.
    pub fn set_waker(&mut self, waker: FetchWaker) {
        self.waker = Some(waker);
    }

    pub fn navigate(&mut self, url: &str) -> Result<(), String> {
        let page = self.adapter.open_url(url).map_err(|e| e.message)?;
        self.current_page = Some(page);
        self.run_root_scripts();
        // Spec: HTML Living Standard §7.4 — scroll to the fragment anchor after navigation.
        // https://html.spec.whatwg.org/multipage/browsing-the-web.html#scroll-to-fragid
        self.apply_anchor_scroll_for(url);
        Ok(())
    }

    pub fn back(&mut self) -> Result<(), String> {
        let page = self.adapter.back().map_err(|e| e.message)?;
        self.current_page = Some(page);
        self.run_root_scripts();
        Ok(())
    }

    pub fn forward(&mut self) -> Result<(), String> {
        let page = self.adapter.forward().map_err(|e| e.message)?;
        self.current_page = Some(page);
        self.run_root_scripts();
        Ok(())
    }

    pub fn reload(&mut self) -> Result<(), String> {
        let page = self.adapter.reload().map_err(|e| e.message)?;
        self.current_page = Some(page);
        self.run_root_scripts();
        Ok(())
    }

    pub fn activate_link(
        &mut self,
        frame_id: &str,
        href: &str,
        target: Option<&str>,
    ) -> Result<(), String> {
        let page = self
            .adapter
            .activate_link(frame_id, href, target)
            .map_err(|e| e.message)?;
        self.current_page = Some(page);
        self.run_root_scripts();
        // Spec: HTML Living Standard §7.4 — scroll to the fragment anchor after navigation.
        // https://html.spec.whatwg.org/multipage/browsing-the-web.html#scroll-to-fragid
        self.apply_anchor_scroll_for(href);
        Ok(())
    }

    /// Build a LivePage for every content frame (root + frameset children),
    /// run its scripts, and splice the resulting scene into that frame (first
    /// paint). Frames without html_content stay on the static path.
    fn run_root_scripts(&mut self) {
        self.live_frames.clear();
        let Some(page) = &self.current_page else {
            return;
        };
        let mut docs = Vec::new();
        collect_frame_documents(&page.root_frame, &mut docs);
        for (frame_id, url, html, rect) in docs {
            let (live, scene) = LivePage::load(&url, &html, &rect, self.waker.clone());
            self.splice_frame_scene(&frame_id, &scene.scene_items);
            self.live_frames.push(LiveFrame {
                frame_id,
                page: live,
                rect,
            });
        }
    }

    /// Replace the identified frame's scene_items + paint_commands with `items`.
    fn splice_frame_scene(&mut self, frame_id: &str, items: &[SceneItem]) {
        let Some(page) = &mut self.current_page else {
            return;
        };
        if let Some(frame) = find_frame_mut(&mut page.root_frame, frame_id) {
            let (list, _errors) = scene_items_to_paint_commands(items);
            frame.scene_items = items.to_vec();
            frame.paint_commands = list.commands;
        }
    }

    /// Whether the current page has outstanding async work (fetch/XHR) that a
    /// later `pump_progressive` may resolve into new content.
    pub fn has_pending_async(&self) -> bool {
        self.live_frames.iter().any(|f| f.page.has_pending_work())
    }

    /// Drain settled async work on every live frame, re-lay-out, and splice the
    /// updated scenes. Returns true if any frame was updated (caller repaints).
    pub fn pump_progressive(&mut self) -> bool {
        let mut updates: Vec<(String, Vec<SceneItem>)> = Vec::new();
        for frame in &mut self.live_frames {
            if !frame.page.has_pending_work() {
                continue;
            }
            // Only re-splice/repaint if the pump actually changed the DOM.
            if let Some(scene) = frame.page.pump_and_relayout(&frame.rect) {
                updates.push((frame.frame_id.clone(), scene.scene_items));
            }
        }
        let changed = !updates.is_empty();
        for (frame_id, items) in updates {
            self.splice_frame_scene(&frame_id, &items);
        }
        changed
    }

    /// Whether any live frame has an ongoing animation (queued timers/rAF) the
    /// GUI should keep driving frames for.
    pub fn has_pending_animation(&self) -> bool {
        self.live_frames.iter().any(|f| f.page.has_pending_animation())
    }

    /// Advance one animation frame on every live frame, splicing any updated
    /// scenes. Returns true if any frame changed (caller repaints).
    pub fn animation_frame(&mut self) -> bool {
        let mut updates: Vec<(String, Vec<SceneItem>)> = Vec::new();
        for frame in &mut self.live_frames {
            if !frame.page.has_pending_animation() {
                continue;
            }
            if let Some(scene) = frame.page.animation_frame(&frame.rect) {
                updates.push((frame.frame_id.clone(), scene.scene_items));
            }
        }
        let changed = !updates.is_empty();
        for (frame_id, items) in updates {
            self.splice_frame_scene(&frame_id, &items);
        }
        changed
    }

    /// Pump until async work settles or `max` iterations elapse (headless
    /// screenshots are one-shot and must capture the settled page). Returns
    /// whether any update occurred.
    pub fn settle_async(&mut self, max: usize) -> bool {
        let mut updated = false;
        let mut i = 0;
        while self.has_pending_async() && i < max {
            std::thread::sleep(std::time::Duration::from_millis(10));
            if self.pump_progressive() {
                updated = true;
            }
            i += 1;
        }
        // Drive any JS animations (rAF/setInterval) forward so a one-shot
        // screenshot captures a progressed/settled animation state (bounded —
        // an endless animation stops at the frame cap).
        let mut frames = 0;
        while self.has_pending_animation() && frames < max {
            if self.animation_frame() {
                updated = true;
            }
            frames += 1;
        }
        updated
    }

    /// Notify the layout engine of a new viewport size and return the
    /// re-laid-out page.  Called on every window resize so that block widths,
    /// inline wrapping, and background rects are all recomputed against the
    /// new available width.
    ///
    /// Spec: CSS2.2 §10.1 — the containing block for the initial block
    /// formatting context is the viewport.
    /// https://www.w3.org/TR/CSS22/visudet.html#containing-block-details
    pub fn set_viewport(&mut self, width: u32, height: u32) -> Result<(), String> {
        // Only call the backend when a page is already loaded; ignore resize
        // events that arrive before the first navigation.
        if self.current_page.is_none() {
            return Ok(());
        }
        let page = self
            .adapter
            .set_viewport(width as i64, height as i64)
            .map_err(|e| e.message)?;
        self.current_page = Some(page);
        // The static DTO from the adapter replaced our scripted scenes; reflow
        // each retained LivePage at its new rect and splice it back (a resize
        // reflows — it does NOT re-run scripts).
        let mut updates: Vec<(String, Vec<SceneItem>)> = Vec::new();
        for frame in &mut self.live_frames {
            // Look up this frame's fresh rect from the new static DTO.
            let new_rect = self
                .current_page
                .as_ref()
                .and_then(|p| find_frame(&p.root_frame, &frame.frame_id))
                .map(|f| FrameRect {
                    x: f.rect.x,
                    y: f.rect.y,
                    width: f.rect.width,
                    height: f.rect.height,
                })
                .unwrap_or_else(|| frame.rect.clone());
            frame.rect = new_rect.clone();
            updates.push((frame.frame_id.clone(), frame.page.relayout(&new_rect).scene_items));
        }
        for (frame_id, items) in updates {
            self.splice_frame_scene(&frame_id, &items);
        }
        Ok(())
    }

    pub fn current_url(&self) -> String {
        self.current_page
            .as_ref()
            .map(|p| p.current_url.clone())
            .unwrap_or_default()
    }

    pub fn current_title(&self) -> String {
        self.current_page
            .as_ref()
            .map(|p| p.title.clone())
            .unwrap_or_default()
    }

    /// Returns whether the session has a previous document to go back to.
    pub fn can_go_back(&self) -> bool {
        self.adapter
            .get_navigation_state()
            .map(|s| s.can_back)
            .unwrap_or(false)
    }

    /// Returns whether the session has a forward document to navigate to.
    pub fn can_go_forward(&self) -> bool {
        self.adapter
            .get_navigation_state()
            .map(|s| s.can_forward)
            .unwrap_or(false)
    }

    /// Returns the total pixel height of all frame content by scanning
    /// paint commands across the root frame and all child frames.
    /// For frameset pages the root frame itself may have no paint commands;
    /// all visible content lives in the child frames.
    pub fn content_height(&self) -> i64 {
        let Some(page) = &self.current_page else {
            return 0;
        };
        max_content_height(&page.root_frame)
    }

    /// Collect frameset border rectangles (x, y, width, height) by detecting
    /// gaps between adjacent child frame rects.  Gaps arise because
    /// `FramesetSpec::child_rects` reserves `FRAMESET_BORDER_WIDTH` pixels
    /// between each pair of frames.
    pub fn collect_frameset_borders(&self) -> Vec<(i64, i64, i64, i64)> {
        let Some(page) = &self.current_page else {
            return Vec::new();
        };
        let mut borders = Vec::new();
        collect_borders(&page.root_frame, &mut borders);
        borders
    }

    /// Collect all paint commands from all frames (root + children).
    /// Returns `(frame_id, frame_url, commands)` so the renderer can resolve
    /// relative image URLs against the frame's own document URL.
    pub fn collect_paint_commands(&self) -> Vec<(String, String, Vec<PaintCommand>)> {
        let Some(page) = &self.current_page else {
            return Vec::new();
        };
        let mut result = Vec::new();
        collect_frame_commands(&page.root_frame, &mut result);
        result
    }

    /// Returns the scroll-Y offset set by the most recent anchor-scroll, in
    /// CSS pixels.  Zero if no anchor was found or no page is loaded.
    pub fn anchor_scroll_y(&self) -> i64 {
        self.current_page
            .as_ref()
            .map(|p| p.root_frame.scroll_position.y)
            .unwrap_or(0)
    }

    /// Scan the current page's paint commands for a `DrawRect` whose
    /// `anchor_id` matches the fragment of `url_or_href`, then set the root
    /// frame's `scroll_position.y` to that rect's top edge.
    ///
    /// Spec: HTML Living Standard §7.4 — scrolling to a fragment.
    /// https://html.spec.whatwg.org/multipage/browsing-the-web.html#scroll-to-fragid
    fn apply_anchor_scroll_for(&mut self, url_or_href: &str) {
        let Some(fragment) = fragment_from_url(url_or_href) else {
            return;
        };
        if fragment.is_empty() {
            return;
        }
        let Some(page) = &mut self.current_page else {
            return;
        };
        if let Some(y) = scroll_y_for_anchor(&page.root_frame, fragment) {
            page.root_frame.scroll_position.y = y;
        }
    }
}

/// Collect (frame_id, document_url, html, rect) for every frame in the tree
/// that has html_content, so each can be script-hosted by its own LivePage.
fn collect_frame_documents(
    frame: &BrowserFrameDto,
    out: &mut Vec<(String, String, String, FrameRect)>,
) {
    if let Some(html) = &frame.html_content {
        out.push((
            frame.id.clone(),
            frame.document_url.clone(),
            html.clone(),
            FrameRect {
                x: frame.rect.x,
                y: frame.rect.y,
                width: frame.rect.width,
                height: frame.rect.height,
            },
        ));
    }
    for child in &frame.child_frames {
        collect_frame_documents(child, out);
    }
}

/// Find a frame by id in the tree (immutable).
fn find_frame<'a>(frame: &'a BrowserFrameDto, frame_id: &str) -> Option<&'a BrowserFrameDto> {
    if frame.id == frame_id {
        return Some(frame);
    }
    for child in &frame.child_frames {
        if let Some(f) = find_frame(child, frame_id) {
            return Some(f);
        }
    }
    None
}

/// Find a frame by id in the tree (mutable).
fn find_frame_mut<'a>(
    frame: &'a mut BrowserFrameDto,
    frame_id: &str,
) -> Option<&'a mut BrowserFrameDto> {
    if frame.id == frame_id {
        return Some(frame);
    }
    for child in &mut frame.child_frames {
        if let Some(f) = find_frame_mut(child, frame_id) {
            return Some(f);
        }
    }
    None
}

/// Detect frameset border gaps between adjacent child frames and append
/// their rects to `out` as (x, y, width, height) tuples.
/// A gap is present when adjacent frames do not abut exactly.
fn collect_borders(frame: &BrowserFrameDto, out: &mut Vec<(i64, i64, i64, i64)>) {
    let children = &frame.child_frames;
    if children.len() >= 2 {
        for i in 0..children.len() - 1 {
            let a = &children[i].rect;
            let b = &children[i + 1].rect;
            // Vertical border (cols-based frameset): frames share the same y/height.
            if a.y == b.y && a.height == b.height {
                let gap_x = a.x + a.width;
                let gap_w = b.x - gap_x;
                if gap_w > 0 {
                    out.push((gap_x, a.y, gap_w, a.height));
                }
            }
            // Horizontal border (rows-based frameset): frames share the same x/width.
            if a.x == b.x && a.width == b.width {
                let gap_y = a.y + a.height;
                let gap_h = b.y - gap_y;
                if gap_h > 0 {
                    out.push((a.x, gap_y, a.width, gap_h));
                }
            }
        }
    }
    for child in children {
        collect_borders(child, out);
    }
}

fn collect_frame_commands(frame: &BrowserFrameDto, out: &mut Vec<(String, String, Vec<PaintCommand>)>) {
    // Paint commands already have frame-absolute coordinates applied by
    // display_items_to_scene(), so no additional offset is needed here.
    out.push((frame.id.clone(), frame.current_url.clone(), frame.paint_commands.clone()));

    for child in &frame.child_frames {
        collect_frame_commands(child, out);
    }
}

/// Recursively compute the maximum content bottom-edge across a frame and all
/// its children.  Frame-absolute coordinates are already baked into paint
/// commands by `display_items_to_scene()`, so no offset adjustment is needed.
fn max_content_height(frame: &BrowserFrameDto) -> i64 {
    let local = frame
        .paint_commands
        .iter()
        .map(|cmd| match cmd {
            PaintCommand::DrawRect(r) => r.y + r.height,
            PaintCommand::DrawText(t) => t.y + t.font_px,
            PaintCommand::DrawImage(i) => i.y + i.height,
        })
        .max()
        .unwrap_or(0);

    frame
        .child_frames
        .iter()
        .fold(local, |acc, child| acc.max(max_content_height(child)))
}

/// Extract the fragment identifier (the part after `#`) from a URL or
/// a bare href such as `"#section"`.  Returns `None` when no `#` is present.
fn fragment_from_url(url_or_href: &str) -> Option<&str> {
    url_or_href.find('#').map(|pos| &url_or_href[pos + 1..])
}

/// Search the root frame's paint commands for the first `DrawRect` whose
/// `anchor_id` equals `anchor`.  Returns the rect's `y` coordinate on a match.
fn scroll_y_for_anchor(frame: &BrowserFrameDto, anchor: &str) -> Option<i64> {
    frame.paint_commands.iter().find_map(|cmd| {
        if let PaintCommand::DrawRect(r) = cmd {
            if r.anchor_id.as_deref() == Some(anchor) {
                return Some(r.y);
            }
        }
        None
    })
}
