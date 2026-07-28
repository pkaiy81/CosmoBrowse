/// Link click detection from mouse coordinates.

pub struct HitRegion {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
    pub href: String,
    pub target: Option<String>,
    pub frame_id: String,
}

pub fn hit_test(regions: &[HitRegion], mouse_x: i64, mouse_y: i64) -> Option<&HitRegion> {
    // Search in reverse order (topmost drawn element first).
    regions.iter().rev().find(|region| {
        mouse_x >= region.x
            && mouse_x < region.x + region.width
            && mouse_y >= region.y
            && mouse_y < region.y + region.height
    })
}
