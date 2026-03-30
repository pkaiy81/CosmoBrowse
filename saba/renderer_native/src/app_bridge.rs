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

    /// Collect all paint commands from all frames (root + children), with
    /// frame offsets applied.
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
    // Apply frame rect offset to each paint command.
    let offset_x = frame.rect.x;
    let offset_y = frame.rect.y;

    let adjusted: Vec<PaintCommand> = frame
        .paint_commands
        .iter()
        .map(|cmd| offset_command(cmd, offset_x, offset_y))
        .collect();

    out.push((frame.id.clone(), adjusted));

    for child in &frame.child_frames {
        collect_frame_commands(child, out);
    }
}

fn offset_command(cmd: &PaintCommand, dx: i64, dy: i64) -> PaintCommand {
    match cmd {
        PaintCommand::DrawRect(r) => PaintCommand::DrawRect(cosmo_core::paint_commands::DrawRect {
            x: r.x + dx,
            y: r.y + dy,
            width: r.width,
            height: r.height,
            background_color: r.background_color.clone(),
            opacity: r.opacity,
            z_index: r.z_index,
            clip_rect: r.clip_rect.map(|(cx, cy, cw, ch)| (cx + dx, cy + dy, cw, ch)),
        }),
        PaintCommand::DrawText(t) => PaintCommand::DrawText(cosmo_core::paint_commands::DrawText {
            x: t.x + dx,
            y: t.y + dy,
            text: t.text.clone(),
            color: t.color.clone(),
            font_px: t.font_px,
            font_family: t.font_family.clone(),
            underline: t.underline,
            opacity: t.opacity,
            href: t.href.clone(),
            target: t.target.clone(),
            z_index: t.z_index,
            clip_rect: t.clip_rect.map(|(cx, cy, cw, ch)| (cx + dx, cy + dy, cw, ch)),
        }),
        PaintCommand::DrawImage(img) => PaintCommand::DrawImage(cosmo_core::paint_commands::DrawImage {
            x: img.x + dx,
            y: img.y + dy,
            width: img.width,
            height: img.height,
            src: img.src.clone(),
            alt: img.alt.clone(),
            opacity: img.opacity,
            href: img.href.clone(),
            target: img.target.clone(),
            z_index: img.z_index,
            clip_rect: img.clip_rect.map(|(cx, cy, cw, ch)| (cx + dx, cy + dy, cw, ch)),
        }),
    }
}
