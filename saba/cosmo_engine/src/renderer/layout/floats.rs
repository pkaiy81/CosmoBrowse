//! Float geometry for a block formatting context (Phase 2.3).
//!
//! A BFC tracks the floats placed in it so far; everything else consults that
//! state through two questions:
//!
//! * *where does this float go?* — [`FloatContext::place`], which slides the box
//!   down until a band is wide enough, then packs it against the edge;
//! * *how much room is left at this Y?* — [`FloatContext::band`], which shortens
//!   the available width for a block box or a line box.
//!
//! Keeping it pure (no layout-tree access) is deliberate: the two consumers —
//! block placement and, later, line-box construction — need the identical
//! answer, and this is the piece that can be exhaustively unit-tested on its
//! own before either is wired up.
//!
//! Spec: CSS 2.2 §9.5 — floats. https://www.w3.org/TR/CSS22/visuren.html#floats

use crate::renderer::layout::computed_style::{Clear, Float};
use std::vec::Vec;

/// A float already placed in this formatting context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlacedFloat {
    pub side: FloatSide,
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

impl PlacedFloat {
    fn bottom(&self) -> i64 {
        self.y + self.height
    }

    fn right(&self) -> i64 {
        self.x + self.width
    }

    /// Whether this float overlaps the vertical range `[top, bottom)`. A
    /// zero-height float occupies no band (CSS 2.2 §9.5: it still affects
    /// nothing horizontally).
    fn spans(&self, top: i64, bottom: i64) -> bool {
        self.height > 0 && self.y < bottom.max(top + 1) && self.bottom() > top
    }
}

/// Which edge a float packs against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatSide {
    Left,
    Right,
}

impl FloatSide {
    /// `None` for `float: none` — the box stays in normal flow.
    pub fn from_float(value: Float) -> Option<Self> {
        match value {
            Float::None => None,
            Float::Left => Some(Self::Left),
            Float::Right => Some(Self::Right),
        }
    }
}

/// The horizontal room left between the floats at some Y.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Band {
    /// Content-box-relative left edge of the free space.
    pub left: i64,
    /// Content-box-relative right edge (exclusive).
    pub right: i64,
}

impl Band {
    pub fn width(&self) -> i64 {
        (self.right - self.left).max(0)
    }
}

/// The floats placed in one block formatting context. Coordinates are relative
/// to the BFC root's content box, so a context can be reused wherever that
/// origin is.
#[derive(Debug, Clone, Default)]
pub struct FloatContext {
    /// Width of the containing block's content box.
    content_width: i64,
    floats: Vec<PlacedFloat>,
}

impl FloatContext {
    pub fn new(content_width: i64) -> Self {
        Self {
            content_width: content_width.max(0),
            floats: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.floats.is_empty()
    }

    pub fn placed(&self) -> &[PlacedFloat] {
        &self.floats
    }

    /// The free horizontal band across the vertical range `[top, top+height)`.
    /// A box that tall must fit between these edges.
    pub fn band(&self, top: i64, height: i64) -> Band {
        let bottom = top + height.max(1);
        let mut band = Band {
            left: 0,
            right: self.content_width,
        };
        for float in &self.floats {
            if !float.spans(top, bottom) {
                continue;
            }
            match float.side {
                FloatSide::Left => band.left = band.left.max(float.right()),
                FloatSide::Right => band.right = band.right.min(float.x),
            }
        }
        if band.right < band.left {
            band.right = band.left;
        }
        band
    }

    /// Place a float of `width` × `height` no higher than `top`, honouring
    /// `clear`. Returns its position; the caller lays the box out there and the
    /// context remembers it.
    ///
    /// Spec: CSS 2.2 §9.5.1 — a float is placed as high as possible, then as
    /// far to its side as possible, and never overlaps an earlier float.
    pub fn place(
        &mut self,
        side: FloatSide,
        width: i64,
        height: i64,
        top: i64,
        clear: Clear,
    ) -> PlacedFloat {
        let width = width.max(0);
        let mut y = self.clearance(clear, top);
        // Slide down past any Y where the band is too narrow. Each iteration
        // drops to the next float's bottom edge, so this terminates in at most
        // one step per placed float.
        loop {
            let band = self.band(y, height);
            if width <= band.width() || self.next_edge_below(y).is_none() {
                let x = match side {
                    FloatSide::Left => band.left,
                    // Right floats pack against the right edge; a float wider
                    // than the band still starts inside the content box.
                    FloatSide::Right => (band.right - width).max(band.left),
                };
                let placed = PlacedFloat {
                    side,
                    x,
                    y,
                    width,
                    height,
                };
                self.floats.push(placed);
                return placed;
            }
            y = self.next_edge_below(y).expect("checked above");
        }
    }

    /// The Y a box with `clear` must start at, given it would otherwise be at
    /// `top`: below the bottom edge of the floats on the cleared side(s).
    /// Spec: CSS 2.2 §9.5.2. https://www.w3.org/TR/CSS22/visuren.html#flow-control
    pub fn clearance(&self, clear: Clear, top: i64) -> i64 {
        let clears = |side: FloatSide| match (clear, side) {
            (Clear::Both, _) => true,
            (Clear::Left, FloatSide::Left) => true,
            (Clear::Right, FloatSide::Right) => true,
            _ => false,
        };
        self.floats
            .iter()
            .filter(|float| float.height > 0 && clears(float.side))
            .map(PlacedFloat::bottom)
            .fold(top, i64::max)
    }

    /// The lowest edge of the floats below `y`, used to step the search down.
    /// Also how a line box that cannot fit beside the floats finds the next Y
    /// worth trying (CSS 2.2 §9.5).
    pub fn next_edge_below(&self, y: i64) -> Option<i64> {
        self.floats
            .iter()
            .map(PlacedFloat::bottom)
            .filter(|bottom| *bottom > y)
            .min()
    }

    /// Add an already-placed float, as-is. Used to merge a box's own floats
    /// into the context it inherited from an ancestor: both are expressed in
    /// this box's coordinates by then.
    pub fn adopt(&mut self, float: PlacedFloat) {
        self.floats.push(float);
    }

    /// A copy with every float shifted up by `dy`, i.e. re-expressed in the
    /// coordinates of a descendant box that starts `dy` below this context's
    /// origin. Floats belong to their block formatting context, not to the
    /// block that happens to contain the text flowing around them, so a
    /// descendant asks its questions in its own coordinates against this.
    pub fn translated(&self, dy: i64) -> Self {
        Self {
            content_width: self.content_width,
            floats: self
                .floats
                .iter()
                .map(|f| PlacedFloat { y: f.y - dy, ..*f })
                .collect(),
        }
    }

    /// The bottom of the lowest float — a BFC root contains its floats, so this
    /// is the minimum height it must have. Spec: CSS 2.2 §10.6.7.
    pub fn lowest_bottom(&self) -> i64 {
        self.floats.iter().map(PlacedFloat::bottom).max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> FloatContext {
        FloatContext::new(100)
    }

    #[test]
    fn empty_context_offers_the_whole_width() {
        let band = ctx().band(0, 20);
        assert_eq!(band, Band { left: 0, right: 100 });
        assert_eq!(band.width(), 100);
    }

    #[test]
    fn a_left_float_shortens_the_band_only_where_it_spans() {
        let mut c = ctx();
        let placed = c.place(FloatSide::Left, 30, 50, 0, Clear::None);
        assert_eq!((placed.x, placed.y), (0, 0));
        // Alongside the float.
        assert_eq!(c.band(0, 10), Band { left: 30, right: 100 });
        assert_eq!(c.band(49, 1), Band { left: 30, right: 100 });
        // Below it the full width is back.
        assert_eq!(c.band(50, 10), Band { left: 0, right: 100 });
    }

    #[test]
    fn a_right_float_packs_against_the_right_edge() {
        let mut c = ctx();
        let placed = c.place(FloatSide::Right, 40, 20, 0, Clear::None);
        assert_eq!((placed.x, placed.y), (60, 0));
        assert_eq!(c.band(0, 10), Band { left: 0, right: 60 });
    }

    #[test]
    fn floats_stack_along_the_edge_then_drop_when_out_of_room() {
        let mut c = ctx();
        c.place(FloatSide::Left, 40, 20, 0, Clear::None);
        // Second fits beside the first.
        let second = c.place(FloatSide::Left, 40, 20, 0, Clear::None);
        assert_eq!((second.x, second.y), (40, 0));
        // Third does not: it drops below the shallowest float.
        let third = c.place(FloatSide::Left, 40, 20, 0, Clear::None);
        assert_eq!((third.x, third.y), (0, 20));
    }

    #[test]
    fn opposing_floats_squeeze_the_band_from_both_sides() {
        let mut c = ctx();
        c.place(FloatSide::Left, 30, 20, 0, Clear::None);
        c.place(FloatSide::Right, 30, 20, 0, Clear::None);
        assert_eq!(c.band(0, 10), Band { left: 30, right: 70 });
        assert_eq!(c.band(0, 10).width(), 40);
    }

    #[test]
    fn a_float_wider_than_the_band_still_lands_inside_the_content_box() {
        let mut c = ctx();
        c.place(FloatSide::Left, 80, 20, 0, Clear::None);
        // 40 doesn't fit beside 80, so it drops below rather than overlapping.
        let second = c.place(FloatSide::Left, 40, 20, 0, Clear::None);
        assert_eq!((second.x, second.y), (0, 20));
        // Even a float wider than the whole context starts at the left edge.
        let huge = c.place(FloatSide::Right, 200, 10, 40, Clear::None);
        assert_eq!(huge.x, 0);
    }

    #[test]
    fn clear_moves_past_the_matching_side_only() {
        let mut c = ctx();
        c.place(FloatSide::Left, 20, 30, 0, Clear::None);
        c.place(FloatSide::Right, 20, 60, 0, Clear::None);
        assert_eq!(c.clearance(Clear::None, 0), 0);
        assert_eq!(c.clearance(Clear::Left, 0), 30);
        assert_eq!(c.clearance(Clear::Right, 0), 60);
        assert_eq!(c.clearance(Clear::Both, 0), 60);
        // Clearance never pulls a box upwards.
        assert_eq!(c.clearance(Clear::Left, 90), 90);
    }

    #[test]
    fn a_cleared_float_starts_below_the_earlier_ones() {
        let mut c = ctx();
        c.place(FloatSide::Left, 20, 30, 0, Clear::None);
        let cleared = c.place(FloatSide::Left, 20, 10, 0, Clear::Left);
        assert_eq!((cleared.x, cleared.y), (0, 30));
    }

    #[test]
    fn zero_height_floats_occupy_no_band() {
        let mut c = ctx();
        c.place(FloatSide::Left, 50, 0, 0, Clear::None);
        assert_eq!(c.band(0, 10), Band { left: 0, right: 100 });
        assert_eq!(c.clearance(Clear::Both, 0), 0);
    }

    #[test]
    fn translating_re_expresses_floats_for_a_descendant() {
        let mut c = ctx();
        c.place(FloatSide::Left, 30, 50, 0, Clear::None);
        // A box starting 20 below sees the float's remaining 30.
        let inner = c.translated(20);
        assert_eq!(inner.band(0, 10), Band { left: 30, right: 100 });
        assert_eq!(inner.band(29, 1), Band { left: 30, right: 100 });
        assert_eq!(inner.band(30, 10), Band { left: 0, right: 100 });
    }

    #[test]
    fn lowest_bottom_is_the_height_a_bfc_root_must_contain() {
        let mut c = ctx();
        assert_eq!(c.lowest_bottom(), 0);
        c.place(FloatSide::Left, 20, 30, 0, Clear::None);
        c.place(FloatSide::Right, 20, 80, 0, Clear::None);
        assert_eq!(c.lowest_bottom(), 80);
    }
}
