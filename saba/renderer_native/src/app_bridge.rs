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

    /// Returns the scroll-Y offset set by the most recent anchor-scroll, in
    /// CSS pixels.  Zero if no anchor was found or no page is loaded.
    pub fn anchor_scroll_y(&self) -> i64 {
        self.current_page
            .as_ref()
            .map(|p| p.root_frame.scroll_position.y)
            .unwrap_or(0)
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
