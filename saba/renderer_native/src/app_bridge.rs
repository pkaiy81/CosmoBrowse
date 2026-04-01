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
        let page = self
            .adapter
            .open_url(url)
            .map_err(|e| e.message)?;
        self.current_page = Some(page);
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

    /// Collect all paint commands from all frames (root + children).
    pub fn collect_paint_commands(&self) -> Vec<(String, Vec<PaintCommand>)> {
        let Some(page) = &self.current_page else {
            return Vec::new();
        };
        let mut result = Vec::new();
        collect_frame_commands(&page.root_frame, &mut result);
        result
    }
}

fn collect_frame_commands(
    frame: &BrowserFrameDto,
    out: &mut Vec<(String, Vec<PaintCommand>)>,
) {
    // Paint commands already have frame-absolute coordinates applied by
    // display_items_to_scene(), so no additional offset is needed here.
    out.push((frame.id.clone(), frame.paint_commands.clone()));

    for child in &frame.child_frames {
        collect_frame_commands(child, out);
    }
}

