/// Bridge between the native renderer and the browser engine (NativeAdapter).
use adapter_native::{BrowserFrameDto, BrowserPageDto, NativeAdapter};
use cosmo_core::paint_commands::PaintCommand;

pub struct AppBridge {
    adapter: NativeAdapter,
    current_page: Option<BrowserPageDto>,
}

impl AppBridge {
    pub fn new() -> Self {
        Self {
            adapter: NativeAdapter::default(),
            current_page: None,
        }
    }

    pub fn navigate(&mut self, url: &str) -> Result<(), String> {
        let page = self.adapter.open_url(url).map_err(|e| e.message)?;
        self.current_page = Some(page);
        // Spec: HTML Living Standard §7.4 — scroll to the fragment anchor after navigation.
        // https://html.spec.whatwg.org/multipage/browsing-the-web.html#scroll-to-fragid
        self.apply_anchor_scroll_for(url);
        Ok(())
    }

    pub fn back(&mut self) -> Result<(), String> {
        let page = self.adapter.back().map_err(|e| e.message)?;
        self.current_page = Some(page);
        Ok(())
    }

    pub fn forward(&mut self) -> Result<(), String> {
        let page = self.adapter.forward().map_err(|e| e.message)?;
        self.current_page = Some(page);
        Ok(())
    }

    pub fn reload(&mut self) -> Result<(), String> {
        let page = self.adapter.reload().map_err(|e| e.message)?;
        self.current_page = Some(page);
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
        // Spec: HTML Living Standard §7.4 — scroll to the fragment anchor after navigation.
        // https://html.spec.whatwg.org/multipage/browsing-the-web.html#scroll-to-fragid
        self.apply_anchor_scroll_for(href);
        Ok(())
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

    /// Collect all paint commands from all frames (root + children).
    pub fn collect_paint_commands(&self) -> Vec<(String, Vec<PaintCommand>)> {
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

fn collect_frame_commands(frame: &BrowserFrameDto, out: &mut Vec<(String, Vec<PaintCommand>)>) {
    // Paint commands already have frame-absolute coordinates applied by
    // display_items_to_scene(), so no additional offset is needed here.
    out.push((frame.id.clone(), frame.paint_commands.clone()));

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
