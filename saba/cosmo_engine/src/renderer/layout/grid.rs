//! Grid layout, run by the container, in the same two steps as flex: columns
//! before the items are sized, rows after.
//!
//! Spec: CSS Grid 1 §8 (placement) and §11 (track sizing).
//! https://www.w3.org/TR/css-grid-1/#layout-algorithm

use crate::renderer::layout::computed_style::{
    AlignItems, ComputedStyle, GridAutoRepeat, GridLine, GridPlacement, GridTemplate, GridTrack,
    JustifyContent,
};
use crate::renderer::layout::flex::{
    align_content_distribution, collect_items, pin_contents_boxes,
};
use crate::renderer::layout::layout_object::{
    compute_box_model_metrics, LayoutObject, LayoutSize,
};
use std::cell::RefCell;
use std::rc::Rc;

type Node = Rc<RefCell<LayoutObject>>;

/// The most tracks a grid may reach, however its items ask for more.
const MAX_TRACKS: i32 = 1000;

/// An item's cell range, as zero-based track indexes.
#[derive(Debug, Clone)]
pub(crate) struct GridCell {
    node: Node,
    column: (usize, usize),
    row: (usize, usize),
}

/// What step one worked out, kept for step two.
#[derive(Debug, Clone, Default)]
pub(crate) struct GridLayout {
    cells: Vec<GridCell>,
    columns: Vec<i64>,
    row_tracks: Vec<GridTrack>,
}

/// One axis of the explicit grid, with `repeat(auto-*)` expanded.
struct Axis {
    tracks: Vec<GridTrack>,
    names: Vec<Vec<String>>,
    /// Sizes of implicit tracks.
    auto: Vec<GridTrack>,
    /// Indexes of the tracks that came from `auto-fit`.
    fit: Option<(usize, usize)>,
}

fn track_min_px(track: &GridTrack, available: Option<i64>) -> f64 {
    match track {
        GridTrack::Px(px) => *px,
        GridTrack::Percent(ratio) => available.map(|a| a as f64 * ratio).unwrap_or(0.0),
        GridTrack::MinMax(min, max) => {
            let m = track_min_px(min, available);
            if m > 0.0 {
                m
            } else {
                track_min_px(max, available)
            }
        }
        GridTrack::FitContent(px) => *px,
        _ => 0.0,
    }
}

fn expand_axis(
    template: Option<&GridTemplate>,
    auto: Option<Rc<Vec<GridTrack>>>,
    available: Option<i64>,
    gap: i64,
) -> Axis {
    let auto = auto.map(|a| (*a).clone()).unwrap_or_default();
    let Some(template) = template else {
        return Axis {
            tracks: Vec::new(),
            names: vec![Vec::new()],
            auto,
            fit: None,
        };
    };
    let mut tracks = template.tracks.clone();
    let mut names = template.line_names.clone();
    let mut fit = None;
    if let Some(GridAutoRepeat { at, tracks: unit, fit: is_fit }) = &template.auto_repeat {
        let fixed: f64 = template
            .tracks
            .iter()
            .map(|t| track_min_px(t, available))
            .sum();
        let unit_size: f64 = unit.iter().map(|t| track_min_px(t, available)).sum();
        // As many copies as fit without overflowing; one when the size or the
        // room isn't known.
        // https://www.w3.org/TR/css-grid-1/#auto-repeat
        let count = match available {
            Some(available) if unit_size > 0.0 => {
                let existing = (template.tracks.len() as i64) * gap;
                let room = available as f64 - fixed - existing as f64 + gap as f64;
                let one = unit_size + (unit.len() as i64 * gap) as f64;
                ((room / one).floor() as i64).clamp(1, MAX_TRACKS as i64) as usize
            }
            _ => 1,
        };
        let at = (*at).min(tracks.len());
        let added: Vec<GridTrack> = unit.iter().cycle().take(unit.len() * count).cloned().collect();
        let added_len = added.len();
        tracks.splice(at..at, added);
        let at_names = (at + 1).min(names.len());
        names.splice(at_names..at_names, std::iter::repeat_n(Vec::new(), added_len));
        if *is_fit {
            fit = Some((at, at + added_len));
        }
    }
    Axis { tracks, names, auto, fit }
}

/// Find the `occurrence`th line called `name`, as a zero-based line index.
fn find_named_line(names: &[Vec<String>], name: &str, occurrence: i32) -> Option<i32> {
    let found: Vec<usize> = names
        .iter()
        .enumerate()
        .filter(|(_, line)| line.iter().any(|n| n == name))
        .map(|(i, _)| i)
        .collect();
    let index = if occurrence > 0 {
        found.get(occurrence as usize - 1)
    } else {
        found.iter().rev().nth((-occurrence) as usize - 1)
    };
    index.map(|i| *i as i32)
}

/// The named area `name` covers these rows and columns, as half-open line ranges.
fn area_rect(areas: &[Vec<String>], name: &str) -> Option<((i32, i32), (i32, i32))> {
    let mut rect: Option<((i32, i32), (i32, i32))> = None;
    for (r, row) in areas.iter().enumerate() {
        for (c, cell) in row.iter().enumerate() {
            if cell == name {
                let (r, c) = (r as i32, c as i32);
                rect = Some(match rect {
                    None => ((r, r + 1), (c, c + 1)),
                    Some((rows, cols)) => (
                        (rows.0.min(r), rows.1.max(r + 1)),
                        (cols.0.min(c), cols.1.max(c + 1)),
                    ),
                });
            }
        }
    }
    rect
}

/// Resolve one end of a placement to a line index, if it names a specific line.
/// `area_line` looks a name up as a grid area on this axis.
fn resolve_line(
    line: &GridLine,
    is_end: bool,
    axis: &Axis,
    area_line: &dyn Fn(&str, bool) -> Option<i32>,
) -> Option<i32> {
    match line {
        GridLine::Number(n) if *n > 0 => Some(n - 1),
        GridLine::Number(n) => Some((axis.tracks.len() as i32 + 1 + n).max(0)),
        GridLine::Name(name, occurrence) => {
            let suffixed = format!("{name}-{}", if is_end { "end" } else { "start" });
            area_line(name, is_end)
                .or_else(|| find_named_line(&axis.names, &suffixed, *occurrence))
                .or_else(|| find_named_line(&axis.names, name, *occurrence))
        }
        _ => None,
    }
}

/// An axis placement resolved as far as it can be without auto-placement:
/// `(start, span)`, with no start meaning it still needs a place.
fn resolve_placement(
    placement: Option<&GridPlacement>,
    axis: &Axis,
    area_line: &dyn Fn(&str, bool) -> Option<i32>,
) -> (Option<i32>, i32) {
    let default = GridPlacement::default();
    let placement = placement.unwrap_or(&default);
    let start = resolve_line(&placement.start, false, axis, area_line);
    let end = resolve_line(&placement.end, true, axis, area_line);
    let span_of = |line: &GridLine| match line {
        GridLine::Span(n) => Some(*n),
        _ => None,
    };
    match (start, end) {
        (Some(s), Some(e)) if e > s => (Some(s), e - s),
        (Some(s), Some(e)) if e < s => (Some(e), s - e),
        (Some(s), Some(_)) => (Some(s), 1),
        (Some(s), None) => (Some(s), span_of(&placement.end).unwrap_or(1)),
        (None, Some(e)) => {
            let span = span_of(&placement.start).unwrap_or(1);
            (Some((e - span).max(0)), span)
        }
        (None, None) => (
            None,
            span_of(&placement.start)
                .or_else(|| span_of(&placement.end))
                .unwrap_or(1),
        ),
    }
}

/// Which cells are taken, growing as items land.
#[derive(Default)]
struct Occupancy {
    taken: Vec<(usize, usize, usize, usize)>,
}

impl Occupancy {
    fn is_free(&self, column: (usize, usize), row: (usize, usize)) -> bool {
        !self
            .taken
            .iter()
            .any(|t| column.0 < t.1 && t.0 < column.1 && row.0 < t.3 && t.2 < row.1)
    }

    fn take(&mut self, column: (usize, usize), row: (usize, usize)) {
        self.taken.push((column.0, column.1, row.0, row.1));
    }
}

/// Assign every item a cell. Fully placed items go first, then those fixed on
/// one axis, then the rest in order.
/// https://www.w3.org/TR/css-grid-1/#auto-placement-algo
fn place_cells(
    items: &[Node],
    style: &ComputedStyle,
    columns: &Axis,
    rows: &Axis,
) -> Vec<GridCell> {
    let areas = style.grid_template_areas();
    let column_area = |name: &str, is_end: bool| -> Option<i32> {
        let (_, cols) = area_rect(areas.as_ref()?, name)?;
        Some(if is_end { cols.1 } else { cols.0 })
    };
    let row_area = |name: &str, is_end: bool| -> Option<i32> {
        let (r, _) = area_rect(areas.as_ref()?, name)?;
        Some(if is_end { r.1 } else { r.0 })
    };

    struct Pending {
        node: Node,
        column: (Option<i32>, i32),
        row: (Option<i32>, i32),
    }
    let mut pending: Vec<Pending> = items
        .iter()
        .map(|node| {
            let b = node.borrow();
            let item_style = b.style();
            let name = item_style.grid_area_name();
            let (column_placement, row_placement) = match name {
                Some(name) => {
                    let lines = GridPlacement {
                        start: GridLine::Name(name.to_string(), 1),
                        end: GridLine::Name(name.to_string(), 1),
                    };
                    (Some(lines.clone()), Some(lines))
                }
                None => (
                    item_style.grid_column().map(|p| (*p).clone()),
                    item_style.grid_row().map(|p| (*p).clone()),
                ),
            };
            Pending {
                node: node.clone(),
                column: resolve_placement(column_placement.as_ref(), columns, &column_area),
                row: resolve_placement(row_placement.as_ref(), rows, &row_area),
            }
        })
        .collect();

    let mut occupancy = Occupancy::default();
    let clamp = |start: i32, span: i32| -> (usize, usize) {
        let start = start.clamp(0, MAX_TRACKS);
        (start as usize, (start + span.clamp(1, MAX_TRACKS)).min(MAX_TRACKS + 1) as usize)
    };
    let mut placed: Vec<Option<GridCell>> = vec![None; pending.len()];
    for (index, item) in pending.iter().enumerate() {
        if let (Some(c), Some(r)) = (item.column.0, item.row.0) {
            let column = clamp(c, item.column.1);
            let row = clamp(r, item.row.1);
            occupancy.take(column, row);
            placed[index] = Some(GridCell { node: item.node.clone(), column, row });
        }
    }

    let flow_column = style.grid_auto_flow_column();
    // The grid is at least as wide as the explicit tracks, the items fixed on
    // that axis, and the widest span still to come.
    let mut width_in_cross = if flow_column { rows.tracks.len() } else { columns.tracks.len() };
    for item in &pending {
        let axis = if flow_column { item.row } else { item.column };
        let reach = axis.0.unwrap_or(0).max(0) as usize + axis.1.max(1) as usize;
        width_in_cross = width_in_cross.max(reach);
    }
    let width_in_cross = width_in_cross.max(1);

    // Items fixed on the flow's major axis first, so the rest flow around them.
    for (index, item) in pending.iter_mut().enumerate() {
        if placed[index].is_some() {
            continue;
        }
        let (major, minor) = if flow_column {
            (item.column, item.row)
        } else {
            (item.row, item.column)
        };
        let Some(major_start) = major.0 else { continue };
        let major_range = clamp(major_start, major.1);
        let mut minor_start = 0usize;
        loop {
            let minor_range = (minor_start, minor_start + minor.1.max(1) as usize);
            let (column, row) = if flow_column {
                (major_range, minor_range)
            } else {
                (minor_range, major_range)
            };
            if occupancy.is_free(column, row) {
                occupancy.take(column, row);
                placed[index] = Some(GridCell { node: item.node.clone(), column, row });
                break;
            }
            minor_start += 1;
        }
    }

    // Everything else in order, each after the last (sparse packing).
    let mut cursor = (0usize, 0usize);
    for (index, item) in pending.iter().enumerate() {
        if placed[index].is_some() {
            continue;
        }
        let (major, minor) = if flow_column {
            (item.column, item.row)
        } else {
            (item.row, item.column)
        };
        let minor_span = minor.1.max(1) as usize;
        let major_span = major.1.max(1) as usize;
        let cell = loop {
            let minor_start = match minor.0 {
                Some(fixed) => fixed.max(0) as usize,
                None => cursor.1,
            };
            if minor.0.is_none() && minor_start + minor_span > width_in_cross && minor_start > 0 {
                cursor = (cursor.0 + 1, 0);
                continue;
            }
            let major_range = (cursor.0, cursor.0 + major_span);
            let minor_range = (minor_start, minor_start + minor_span);
            let (column, row) = if flow_column {
                (major_range, minor_range)
            } else {
                (minor_range, major_range)
            };
            if occupancy.is_free(column, row) {
                cursor.1 = minor_start + minor_span;
                break (column, row);
            }
            if minor.0.is_some() {
                cursor = (cursor.0 + 1, cursor.1);
            } else {
                cursor.1 += 1;
            }
        };
        occupancy.take(cell.0, cell.1);
        placed[index] = Some(GridCell { node: item.node.clone(), column: cell.0, row: cell.1 });
    }
    placed.into_iter().flatten().collect()
}

/// What an item asks of the tracks it spans.
struct Contribution {
    range: (usize, usize),
    min: i64,
    max: i64,
}

fn min_is_intrinsic(track: &GridTrack) -> bool {
    match track {
        GridTrack::Auto | GridTrack::MinContent | GridTrack::MaxContent | GridTrack::Fr(_) => true,
        GridTrack::FitContent(_) => true,
        GridTrack::MinMax(min, _) => min_is_intrinsic(min),
        GridTrack::Px(_) | GridTrack::Percent(_) => false,
    }
}

fn max_is_intrinsic(track: &GridTrack) -> bool {
    match track {
        GridTrack::Auto | GridTrack::MinContent | GridTrack::MaxContent => true,
        GridTrack::FitContent(_) => true,
        GridTrack::MinMax(_, max) => max_is_intrinsic(max),
        GridTrack::Fr(_) | GridTrack::Px(_) | GridTrack::Percent(_) => false,
    }
}

fn is_flexible(track: &GridTrack) -> Option<f64> {
    match track {
        GridTrack::Fr(f) => Some(*f),
        GridTrack::MinMax(_, max) => is_flexible(max),
        _ => None,
    }
}

fn stretches(track: &GridTrack) -> bool {
    match track {
        GridTrack::Auto => true,
        GridTrack::MinMax(_, max) => matches!(**max, GridTrack::Auto),
        _ => false,
    }
}

fn base_size(track: &GridTrack, available: Option<i64>) -> f64 {
    match track {
        GridTrack::Px(px) => *px,
        GridTrack::Percent(ratio) => available.map(|a| a as f64 * ratio).unwrap_or(0.0),
        GridTrack::MinMax(min, _) => base_size(min, available),
        _ => 0.0,
    }
}

/// Upper limit before item contributions: `None` is unbounded for now.
fn growth_limit(track: &GridTrack, available: Option<i64>) -> Option<f64> {
    match track {
        GridTrack::Px(px) => Some(*px),
        GridTrack::Percent(ratio) => available.map(|a| a as f64 * ratio),
        GridTrack::MinMax(_, max) => growth_limit(max, available),
        _ => None,
    }
}

/// Run the track sizing algorithm for one axis, `available` being the container's
/// content size on it if that is known.
/// https://www.w3.org/TR/css-grid-1/#algo-track-sizing
fn size_tracks(
    tracks: &[GridTrack],
    contributions: &[Contribution],
    available: Option<i64>,
    gap: i64,
    stretch: bool,
) -> Vec<i64> {
    let count = tracks.len();
    if count == 0 {
        return Vec::new();
    }
    let mut base: Vec<f64> = tracks.iter().map(|t| base_size(t, available)).collect();
    let mut limit: Vec<Option<f64>> =
        tracks.iter().map(|t| growth_limit(t, available)).collect();

    // Items spanning one track first, then wider ones spread what's still short
    // over the intrinsic tracks they cross.
    let mut by_span: Vec<&Contribution> = contributions.iter().collect();
    by_span.sort_by_key(|c| c.range.1 - c.range.0);
    let wanted_min = |track: &GridTrack, c: &Contribution| match track {
        GridTrack::MaxContent => c.max,
        GridTrack::MinMax(min, _) if matches!(**min, GridTrack::MaxContent) => c.max,
        _ => c.min,
    };
    for c in by_span {
        let span = c.range.1 - c.range.0;
        let crossed = &tracks[c.range.0..c.range.1];
        if span > 1 && crossed.iter().any(|t| is_flexible(t).is_some()) {
            continue;
        }
        let gaps = (gap * (span as i64 - 1)) as f64;
        let intrinsic: Vec<usize> = (c.range.0..c.range.1)
            .filter(|i| min_is_intrinsic(&tracks[*i]))
            .collect();
        if !intrinsic.is_empty() {
            let have: f64 = base[c.range.0..c.range.1].iter().sum::<f64>() + gaps;
            let want = wanted_min(&tracks[intrinsic[0]], c) as f64;
            if want > have {
                let share = (want - have) / intrinsic.len() as f64;
                for i in &intrinsic {
                    base[*i] += share;
                }
            }
        }
        let growable: Vec<usize> = (c.range.0..c.range.1)
            .filter(|i| max_is_intrinsic(&tracks[*i]))
            .collect();
        if !growable.is_empty() {
            let have: f64 = (c.range.0..c.range.1)
                .map(|i| limit[i].unwrap_or(base[i]))
                .sum::<f64>()
                + gaps;
            let want = match &tracks[growable[0]] {
                GridTrack::MinContent => c.min,
                _ => c.max,
            } as f64;
            if want > have {
                let share = (want - have) / growable.len() as f64;
                for i in &growable {
                    let grown = limit[*i].unwrap_or(base[*i]) + share;
                    limit[*i] = Some(grown);
                }
            }
        }
    }
    for i in 0..count {
        if let GridTrack::FitContent(cap) = &tracks[i] {
            limit[i] = Some(limit[i].unwrap_or(0.0).min(*cap).max(base[i]));
        }
        if is_flexible(&tracks[i]).is_none() {
            let settled = limit[i].unwrap_or(base[i]).max(base[i]);
            limit[i] = Some(settled);
        } else {
            limit[i] = None;
        }
    }

    let gaps = (gap * (count as i64 - 1)) as f64;
    // Share what's left among tracks that can still grow.
    let mut free = available.map(|a| a as f64 - gaps - base.iter().sum::<f64>());
    match free.as_mut() {
        Some(free) => {
            for _ in 0..count {
                if *free <= 0.0 {
                    break;
                }
                let growable: Vec<usize> = (0..count)
                    .filter(|i| limit[*i].is_some_and(|l| l > base[*i]))
                    .collect();
                if growable.is_empty() {
                    break;
                }
                let share = *free / growable.len() as f64;
                for i in growable {
                    let room = limit[i].unwrap_or(base[i]) - base[i];
                    let given = share.min(room);
                    base[i] += given;
                    *free -= given;
                }
            }
        }
        None => {
            for i in 0..count {
                if let Some(l) = limit[i] {
                    base[i] = base[i].max(l);
                }
            }
        }
    }

    // Flexible tracks: find the size of one `fr`, treating any track whose
    // content already needs more than its share as fixed.
    // https://www.w3.org/TR/css-grid-1/#algo-find-fr-size
    let flex: Vec<Option<f64>> = tracks.iter().map(is_flexible).collect();
    if flex.iter().any(Option::is_some) {
        let unit = match available {
            Some(available) => {
                let mut fixed: Vec<bool> = flex.iter().map(Option::is_none).collect();
                let mut unit = 0.0;
                for _ in 0..=count {
                    let used: f64 = (0..count).filter(|i| fixed[*i]).map(|i| base[i]).sum();
                    let total_fr: f64 = (0..count)
                        .filter(|i| !fixed[*i])
                        .map(|i| flex[i].unwrap_or(0.0))
                        .sum();
                    unit = (available as f64 - gaps - used).max(0.0) / total_fr.max(1.0);
                    let mut changed = false;
                    for i in 0..count {
                        if !fixed[i] && flex[i].unwrap_or(0.0) * unit < base[i] {
                            fixed[i] = true;
                            changed = true;
                        }
                    }
                    if !changed {
                        break;
                    }
                }
                unit
            }
            // No definite size: one fr is what the largest track needs.
            None => (0..count)
                .filter_map(|i| {
                    let f = flex[i]?;
                    Some(base[i] / f.max(1.0))
                })
                .fold(0.0, f64::max),
        };
        for i in 0..count {
            if let Some(f) = flex[i] {
                base[i] = base[i].max(f * unit);
            }
        }
    } else if stretch {
        if let Some(available) = available {
            let leftover = available as f64 - gaps - base.iter().sum::<f64>();
            let autos: Vec<usize> = (0..count).filter(|i| stretches(&tracks[*i])).collect();
            if leftover > 0.0 && !autos.is_empty() {
                let share = leftover / autos.len() as f64;
                for i in autos {
                    base[i] += share;
                }
            }
        }
    }

    // Round so the rounding errors don't add up across tracks.
    let mut out = Vec::with_capacity(count);
    let mut exact = 0.0;
    let mut whole = 0i64;
    for size in base {
        exact += size;
        let next = exact.round() as i64;
        out.push(next - whole);
        whole = next;
    }
    out
}

/// Fill `tracks` up to `count` with implicit tracks.
fn with_implicit(mut tracks: Vec<GridTrack>, auto: &[GridTrack], count: usize) -> Vec<GridTrack> {
    let mut index = 0;
    while tracks.len() < count {
        tracks.push(if auto.is_empty() {
            GridTrack::Auto
        } else {
            auto[index % auto.len()].clone()
        });
        index += 1;
    }
    tracks
}

/// Step one: place the items and size the columns.
pub(crate) fn assign_sizes(container: &Node) {
    let (style, width, height) = {
        let b = container.borrow();
        let style = b.style();
        let size = b.content_size();
        let height = style.has_author_height().then_some(size.height());
        (style, size.width(), height)
    };
    let items = collect_items(container);
    pin_contents_boxes(container);
    let (column_gap, row_gap) = (style.column_gap(), style.row_gap());

    let mut columns = expand_axis(
        style.grid_columns_template().as_deref(),
        style.grid_auto_columns(),
        Some(width),
        column_gap,
    );
    let mut rows = expand_axis(
        style.grid_rows_template().as_deref(),
        style.grid_auto_rows(),
        height,
        row_gap,
    );
    // Named areas make the explicit grid at least as big as they are.
    if let Some(areas) = style.grid_template_areas() {
        let wide = areas.iter().map(Vec::len).max().unwrap_or(0);
        columns.tracks = with_implicit(columns.tracks, &columns.auto, wide);
        rows.tracks = with_implicit(rows.tracks, &rows.auto, areas.len());
    }
    let mut cells = place_cells(&items, &style, &columns, &rows);

    // `auto-fit` collapses repeated tracks nothing landed in.
    if let Some((from, to)) = columns.fit {
        let used = |track: usize| cells.iter().any(|c| c.column.0 <= track && track < c.column.1);
        let remap: Vec<Option<usize>> = {
            let mut next = 0;
            (0..columns.tracks.len())
                .map(|track| {
                    if track >= from && track < to && !used(track) {
                        None
                    } else {
                        next += 1;
                        Some(next - 1)
                    }
                })
                .collect()
        };
        let mapped = |line: usize| {
            remap[..line.min(remap.len())].iter().flatten().count() + line.saturating_sub(remap.len())
        };
        for cell in &mut cells {
            cell.column = (mapped(cell.column.0), mapped(cell.column.1));
        }
        let mut track = 0;
        columns.tracks.retain(|_| {
            let keep = remap[track].is_some();
            track += 1;
            keep
        });
    }

    let column_count = cells.iter().map(|c| c.column.1).max().unwrap_or(0).max(columns.tracks.len()).max(1);
    let row_count = cells.iter().map(|c| c.row.1).max().unwrap_or(0).max(rows.tracks.len());
    let column_tracks = with_implicit(columns.tracks, &columns.auto, column_count);
    let row_tracks = with_implicit(rows.tracks, &rows.auto, row_count);

    let contributions: Vec<Contribution> = cells
        .iter()
        .filter(|c| column_tracks[c.column.0..c.column.1].iter().any(|t| {
            min_is_intrinsic(t) || max_is_intrinsic(t)
        }))
        .map(|cell| {
            let b = cell.node.borrow();
            let item_style = b.style();
            let metrics = compute_box_model_metrics(&item_style);
            let extra = metrics.outer_horizontal();
            let parent = LayoutSize::new(width, 0);
            let explicit = b.resolved_width_for(parent);
            let (min, max) = if explicit > 0 {
                (explicit, explicit)
            } else if b.is_replaced_box() {
                let w = b.replaced_width_for(parent) as i64;
                (w, w)
            } else if item_style.overflow_clip() || item_style.overflow_scrollable() {
                (0, b.max_content_width_for_flex())
            } else {
                (b.min_content_width_for_flex(), b.max_content_width_for_flex())
            };
            Contribution {
                range: cell.column,
                min: min + extra,
                max: max + extra,
            }
        })
        .collect();
    let stretch = style.justify_content() == JustifyContent::Normal;
    let column_sizes = size_tracks(&column_tracks, &contributions, Some(width), column_gap, stretch);

    let justify_items = style.justify_items().unwrap_or(AlignItems::Stretch);
    for cell in &cells {
        let span_width: i64 = column_sizes[cell.column.0..cell.column.1].iter().sum::<i64>()
            + column_gap * (cell.column.1 - cell.column.0 - 1) as i64;
        let mut b = cell.node.borrow_mut();
        let item_style = b.style();
        let metrics = compute_box_model_metrics(&item_style);
        let room = (span_width - metrics.outer_horizontal()).max(0);
        let parent = LayoutSize::new(span_width, 0);
        let explicit = b.resolved_width_for(parent);
        let auto_margin = item_style.margin_left_auto() || item_style.margin_right_auto();
        b.assigned_width = Some(if explicit > 0 || item_style.explicit_zero_width() {
            explicit
        } else if item_style.justify_self_or(justify_items) == AlignItems::Stretch && !auto_margin {
            room
        } else {
            b.max_content_width_for_flex().min(room)
        });
    }

    container.borrow_mut().grid_layout = Some(GridLayout {
        cells,
        columns: column_sizes,
        row_tracks,
    });
}

/// Step two: size the rows from the items' heights and place everything.
pub(crate) fn place_items(container: &Node, parent_height: i64) {
    let (layout, style) = {
        let b = container.borrow();
        (b.grid_layout.clone(), b.style())
    };
    let Some(layout) = layout else { return };
    let (column_gap, row_gap) = (style.column_gap(), style.row_gap());

    let definite = container.borrow().definite_content_height(parent_height);
    let contributions: Vec<Contribution> = layout
        .cells
        .iter()
        .map(|cell| {
            let b = cell.node.borrow();
            let metrics = compute_box_model_metrics(&b.style());
            let outer = b.size().height() + metrics.margin.top + metrics.margin.bottom;
            Contribution {
                range: cell.row,
                min: outer,
                max: outer,
            }
        })
        .collect();
    let mut rows = size_tracks(&layout.row_tracks, &contributions, definite, row_gap, false);

    let natural: i64 =
        rows.iter().sum::<i64>() + row_gap * (rows.len() as i64 - 1).max(0);
    let height = {
        let mut b = container.borrow_mut();
        let metrics = compute_box_model_metrics(&b.style());
        let inner = metrics.inner_vertical();
        let used = definite.unwrap_or(natural);
        let clamped = b.style().clamp_height(used + inner, parent_height) - inner;
        let width = b.size().width();
        b.size = LayoutSize::new(width, clamped.max(0) + inner);
        clamped.max(0)
    };

    // Room left over goes to the rows per `align-content`; `stretch`, the
    // default, grows the auto ones.
    let free = height - natural;
    let (mut lead, mut between) = (0, 0);
    if free > 0 {
        let stretching: Vec<usize> = (0..rows.len())
            .filter(|i| stretches(&layout.row_tracks[*i]))
            .collect();
        if style.align_content() == crate::renderer::layout::computed_style::AlignContent::Stretch
            && !stretching.is_empty()
        {
            let share = free / stretching.len() as i64;
            for i in stretching {
                rows[i] += share;
            }
        } else {
            (lead, between) =
                align_content_distribution(style.align_content(), free, rows.len() as i64);
        }
    }
    let content_width = container.borrow().content_size().width();
    let used_width: i64 = layout.columns.iter().sum::<i64>()
        + column_gap * (layout.columns.len() as i64 - 1).max(0);
    let (column_lead, column_between) = justify_tracks(
        style.justify_content(),
        content_width - used_width,
        layout.columns.len() as i64,
    );

    let track_start = |sizes: &[i64], gap: i64, lead: i64, between: i64, index: usize| -> i64 {
        lead + sizes[..index].iter().sum::<i64>() + (gap + between) * index as i64
    };
    let align = style.align_items();
    let justify_items = style.justify_items().unwrap_or(AlignItems::Stretch);
    for cell in &layout.cells {
        let x0 = track_start(&layout.columns, column_gap, column_lead, column_between, cell.column.0);
        let y0 = track_start(&rows, row_gap, lead, between, cell.row.0);
        let area_w: i64 = layout.columns[cell.column.0..cell.column.1].iter().sum::<i64>()
            + (column_gap + column_between) * (cell.column.1 - cell.column.0 - 1) as i64;
        let area_h: i64 = rows[cell.row.0..cell.row.1].iter().sum::<i64>()
            + (row_gap + between) * (cell.row.1 - cell.row.0 - 1) as i64;

        let (size, metrics, item_style) = {
            let b = cell.node.borrow();
            (b.size(), compute_box_model_metrics(&b.style()), b.style())
        };
        let room_w = area_w - metrics.margin.left - metrics.margin.right;
        let room_h = area_h - metrics.margin.top - metrics.margin.bottom;
        let x_shift = if item_style.margin_left_auto() && item_style.margin_right_auto() {
            (room_w - size.width()) / 2
        } else if item_style.margin_left_auto() {
            room_w - size.width()
        } else if item_style.margin_right_auto() {
            0
        } else {
            match item_style.justify_self_or(justify_items) {
                AlignItems::Center => (room_w - size.width()) / 2,
                AlignItems::FlexEnd => room_w - size.width(),
                _ => 0,
            }
        };
        let mut y_shift = 0;
        match item_style.align_self_or(align) {
            AlignItems::Stretch => {
                let auto = !item_style.has_author_height() && !cell.node.borrow().is_replaced_box();
                if auto && room_h != size.height() {
                    let stretched = item_style.clamp_height(room_h.max(0), height);
                    let mut b = cell.node.borrow_mut();
                    let width = b.size().width();
                    b.size = LayoutSize::new(width, stretched);
                    b.stretched_height = Some(stretched);
                    let is_flex = b.style().display()
                        == crate::renderer::layout::computed_style::DisplayType::Flex;
                    let is_grid = b.style().display()
                        == crate::renderer::layout::computed_style::DisplayType::Grid;
                    drop(b);
                    if is_flex {
                        crate::renderer::layout::flex::place_items(&cell.node, stretched);
                    } else if is_grid {
                        place_items(&cell.node, stretched);
                    }
                }
            }
            AlignItems::Center => y_shift = (room_h - size.height()) / 2,
            AlignItems::FlexEnd => y_shift = room_h - size.height(),
            _ => {}
        }
        cell.node.borrow_mut().inline_offset =
            Some((x0 + metrics.margin.left + x_shift, y0 + metrics.margin.top + y_shift));
    }
}

fn justify_tracks(justify: JustifyContent, free: i64, count: i64) -> (i64, i64) {
    if free <= 0 || count == 0 {
        return (0, 0);
    }
    match justify {
        JustifyContent::Normal | JustifyContent::FlexStart => (0, 0),
        JustifyContent::FlexEnd => (free, 0),
        JustifyContent::Center => (free / 2, 0),
        JustifyContent::SpaceBetween if count > 1 => (0, free / (count - 1)),
        JustifyContent::SpaceBetween => (0, 0),
        JustifyContent::SpaceAround => (free / count / 2, free / count),
        JustifyContent::SpaceEvenly => (free / (count + 1), free / (count + 1)),
    }
}
