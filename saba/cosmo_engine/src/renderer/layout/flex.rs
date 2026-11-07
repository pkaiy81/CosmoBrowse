//! Flex layout, run by the container.
//!
//! It happens in two steps because of how the size pass walks the tree. Before
//! the children are sized, the container fixes each item's width, since text in
//! the item wraps against it. After they are sized, it knows their heights, and
//! can build the lines' cross sizes, set its own height and place the items.
//!
//! Spec: CSS Flexbox 1 §9. https://www.w3.org/TR/css-flexbox-1/#layout-algorithm

use crate::renderer::layout::computed_style::{
    AlignContent, AlignItems, DisplayType, FlexDirection, FlexWrap, JustifyContent,
};
use crate::renderer::layout::layout_object::{compute_box_model_metrics, LayoutObject};
use std::cell::RefCell;
use std::rc::Rc;

type Node = Rc<RefCell<LayoutObject>>;

/// The items of a flex or grid container in document order: its in-flow child
/// boxes, reaching through `display: contents`.
pub(crate) fn collect_items(container: &Node) -> Vec<Node> {
    let mut items = Vec::new();
    let mut child = container.borrow().first_child();
    while let Some(c) = child {
        let (next, contents, skip) = {
            let b = c.borrow();
            (
                b.next_sibling(),
                b.style().display() == DisplayType::Contents,
                b.is_out_of_flow_box() || b.is_whitespace_text(),
            )
        };
        if contents {
            items.extend(collect_items(&c));
        } else if !skip {
            items.push(c);
        }
        child = next;
    }
    items
}

/// Fill the room a `display: contents` box has in layout: nothing, but its
/// children are placed relative to its origin, so that has to be the container's.
pub(crate) fn pin_contents_boxes(container: &Node) {
    let mut child = container.borrow().first_child();
    while let Some(c) = child {
        let next = c.borrow().next_sibling();
        if c.borrow().style().display() == DisplayType::Contents {
            c.borrow_mut().inline_offset = Some((0, 0));
            pin_contents_boxes(&c);
        } else if c.borrow().is_out_of_flow_box() {
            c.borrow_mut().flow_offset_y = Some(0);
        }
        child = next;
    }
}

fn ordered_items(container: &Node) -> Vec<Node> {
    let mut items = collect_items(container);
    items.sort_by_key(|item| item.borrow().style().order());
    items
}

struct FlexItem {
    node: Node,
    /// Main size of the content box before flexing.
    base: f64,
    /// Margins, padding and border along the main axis.
    outer_extra: f64,
    /// Smallest and largest the content box may be.
    min: f64,
    max: f64,
    grow: f64,
    shrink: f64,
    target: f64,
    frozen: bool,
}

impl FlexItem {
    fn hypothetical(&self) -> f64 {
        self.base.clamp(self.min, self.max)
    }

    fn hypothetical_outer(&self) -> f64 {
        self.hypothetical() + self.outer_extra
    }
}

fn row_item(node: &Node, container_width: i64, gap_free_width: i64) -> FlexItem {
    let b = node.borrow();
    let style = b.style();
    let metrics = compute_box_model_metrics(&style);
    let inner = metrics.inner_horizontal() as f64;
    let parent = crate::renderer::layout::layout_object::LayoutSize::new(container_width, 0);
    let replaced = b.is_replaced_box();
    let explicit = b.resolved_width_for(parent) as f64;
    let base = if let Some(ratio) = style.flex_basis_ratio() {
        (container_width as f64 * ratio - inner).max(0.0)
    } else if let Some(basis) = style.flex_basis() {
        if style.is_border_box() {
            (basis - inner).max(0.0)
        } else {
            basis
        }
    } else if explicit > 0.0 || style.explicit_zero_width() {
        explicit
    } else if replaced {
        (b.replaced_width_for(parent) - inner).max(0.0)
    } else {
        b.max_content_width_for_flex() as f64
    };
    // `min-width: auto`: an item won't shrink below its content unless it clips.
    let limited = |content: f64| {
        (style.clamp_width((content + inner) as i64, gap_free_width) as f64 - inner).max(0.0)
    };
    let automatic_min = if style.overflow_clip() || style.overflow_scrollable() {
        0.0
    } else if explicit > 0.0 {
        explicit.min(b.min_content_width_for_flex() as f64)
    } else if replaced {
        0.0
    } else {
        b.min_content_width_for_flex() as f64
    };
    let min = limited(0.0).max(automatic_min);
    let max = if style.has_max_width() {
        limited(f64::from(i32::MAX)).max(min)
    } else {
        f64::INFINITY
    };
    FlexItem {
        node: node.clone(),
        base,
        outer_extra: inner + metrics.margin.left as f64 + metrics.margin.right as f64,
        min,
        max,
        grow: style.flex_grow(),
        shrink: style.flex_shrink(),
        target: 0.0,
        frozen: false,
    }
}

/// Resolve flexible lengths for one line.
/// https://www.w3.org/TR/css-flexbox-1/#resolve-flexible-lengths
fn resolve_lengths(line: &mut [FlexItem], available: f64, gaps: f64) {
    let outer_hypothetical: f64 = line.iter().map(|i| i.hypothetical_outer()).sum();
    let grow = outer_hypothetical + gaps < available;
    for item in line.iter_mut() {
        item.target = item.hypothetical();
        let factor = if grow { item.grow } else { item.shrink };
        item.frozen = factor == 0.0
            || (grow && item.base > item.hypothetical())
            || (!grow && item.base < item.hypothetical());
    }
    // Each round freezes at least one item, or ends.
    for _ in 0..=line.len() {
        if line.iter().all(|i| i.frozen) {
            break;
        }
        let used: f64 = line
            .iter()
            .map(|i| {
                i.outer_extra + if i.frozen { i.target } else { i.base }
            })
            .sum();
        let mut free = available - gaps - used;
        // Factors summing below 1 hand out only that share of the space.
        let factors: f64 = line
            .iter()
            .filter(|i| !i.frozen)
            .map(|i| if grow { i.grow } else { i.shrink })
            .sum();
        if factors < 1.0 {
            free *= factors;
        }
        let scaled_shrink: f64 = line
            .iter()
            .filter(|i| !i.frozen)
            .map(|i| i.shrink * i.base)
            .sum();
        let mut total_violation = 0.0;
        let mut violations = Vec::with_capacity(line.len());
        for item in line.iter_mut() {
            if item.frozen {
                violations.push(0.0);
                continue;
            }
            let unclamped = if grow {
                item.base + free * item.grow / factors
            } else if scaled_shrink > 0.0 {
                item.base + free * (item.shrink * item.base) / scaled_shrink
            } else {
                item.base
            };
            let clamped = unclamped.clamp(item.min, item.max);
            item.target = clamped;
            total_violation += clamped - unclamped;
            violations.push(clamped - unclamped);
        }
        for (item, violation) in line.iter_mut().zip(violations) {
            if item.frozen {
                continue;
            }
            if total_violation == 0.0
                || (total_violation > 0.0 && violation > 0.0)
                || (total_violation < 0.0 && violation < 0.0)
            {
                item.frozen = true;
            }
        }
    }
}

/// Step one: fix each item's width, and break a row into lines.
pub(crate) fn assign_sizes(container: &Node) {
    let (direction, wrap, width, gap) = {
        let b = container.borrow();
        let style = b.style();
        (
            style.flex_direction(),
            style.flex_wrap(),
            b.content_size().width(),
            style.column_gap(),
        )
    };
    let nodes = ordered_items(container);
    pin_contents_boxes(container);
    let mut lines: Vec<Vec<Node>> = Vec::new();
    match direction {
        FlexDirection::Row => {
            let mut items: Vec<FlexItem> =
                nodes.iter().map(|n| row_item(n, width, width)).collect();
            let mut line: Vec<FlexItem> = Vec::new();
            let mut used = 0.0;
            let flush = |line: &mut Vec<FlexItem>, lines: &mut Vec<Vec<Node>>| {
                if line.is_empty() {
                    return;
                }
                let gaps = (gap * (line.len() as i64 - 1)) as f64;
                resolve_lengths(line, width as f64, gaps);
                for item in line.iter() {
                    let mut b = item.node.borrow_mut();
                    b.assigned_width = Some(item.target.round() as i64);
                }
                lines.push(line.iter().map(|i| i.node.clone()).collect());
                line.clear();
            };
            for item in items.drain(..) {
                let outer = item.hypothetical_outer();
                let gap_before = if line.is_empty() { 0.0 } else { gap as f64 };
                if wrap != FlexWrap::NoWrap
                    && !line.is_empty()
                    && used + gap_before + outer > width as f64
                {
                    flush(&mut line, &mut lines);
                    used = outer;
                } else {
                    used += gap_before + outer;
                }
                line.push(item);
            }
            flush(&mut line, &mut lines);
        }
        FlexDirection::Column => {
            let align = container.borrow().style().align_items();
            for node in &nodes {
                let mut b = node.borrow_mut();
                let style = b.style();
                let metrics = compute_box_model_metrics(&style);
                let parent =
                    crate::renderer::layout::layout_object::LayoutSize::new(width, 0);
                let room = (width - metrics.outer_horizontal()).max(0);
                let explicit = b.resolved_width_for(parent);
                let self_align = style.align_self_or(align);
                let auto_margin = style.margin_left_auto() || style.margin_right_auto();
                let assigned = if explicit > 0 || style.explicit_zero_width() {
                    explicit
                } else if self_align == AlignItems::Stretch && !auto_margin {
                    room
                } else {
                    (b.max_content_width_for_flex()).min(room)
                };
                b.assigned_width = Some(assigned);
            }
            if !nodes.is_empty() {
                lines.push(nodes);
            }
        }
    }
    container.borrow_mut().flex_lines = lines;
}

/// How a main or cross axis's leftover space is shared: (before the first box,
/// between each pair of boxes).
fn distribute(justify: JustifyContent, free: i64, count: i64) -> (i64, i64) {
    if free <= 0 {
        return match justify {
            JustifyContent::Center => (free / 2, 0),
            JustifyContent::FlexEnd => (free, 0),
            _ => (0, 0),
        };
    }
    match justify {
        JustifyContent::FlexStart | JustifyContent::Normal => (0, 0),
        JustifyContent::FlexEnd => (free, 0),
        JustifyContent::Center => (free / 2, 0),
        JustifyContent::SpaceBetween if count > 1 => (0, free / (count - 1)),
        JustifyContent::SpaceBetween => (0, 0),
        JustifyContent::SpaceAround => (free / count / 2, free / count),
        JustifyContent::SpaceEvenly => (free / (count + 1), free / (count + 1)),
    }
}

pub(crate) fn align_content_distribution(align: AlignContent, free: i64, count: i64) -> (i64, i64) {
    let justify = match align {
        AlignContent::Stretch | AlignContent::FlexStart => JustifyContent::FlexStart,
        AlignContent::FlexEnd => JustifyContent::FlexEnd,
        AlignContent::Center => JustifyContent::Center,
        AlignContent::SpaceBetween => JustifyContent::SpaceBetween,
        AlignContent::SpaceAround => JustifyContent::SpaceAround,
        AlignContent::SpaceEvenly => JustifyContent::SpaceEvenly,
    };
    distribute(justify, free, count)
}

/// The container's content height: `Some` when something other than its
/// contents sets it, along with its height once contents are known.
fn settle_height(container: &Node, parent_height: i64, content_height: i64) -> (i64, bool) {
    let mut b = container.borrow_mut();
    let metrics = compute_box_model_metrics(&b.style());
    let inner = metrics.inner_vertical();
    let declared = b.definite_content_height(parent_height);
    let auto_height = declared.unwrap_or(content_height);
    let style = b.style();
    let clamped = style.clamp_height(auto_height + inner, parent_height) - inner;
    let width = b.size().width();
    b.size = crate::renderer::layout::layout_object::LayoutSize::new(width, clamped.max(0) + inner);
    let definite = declared.is_some() || clamped != auto_height;
    (clamped.max(0), definite)
}

/// Step two: with the items' heights known, set the container's height and place
/// everything. Also re-run when a parent stretches this container, so it takes
/// the container's current height as given.
pub(crate) fn place_items(container: &Node, parent_height: i64) {
    let direction = container.borrow().style().flex_direction();
    match direction {
        FlexDirection::Row => place_row(container, parent_height),
        FlexDirection::Column => place_column(container, parent_height),
    }
}

/// An item given a height by its container may be a container too.
fn relayout_after_stretch(item: &Node, height: i64) {
    {
        let mut b = item.borrow_mut();
        let width = b.size().width();
        b.size = crate::renderer::layout::layout_object::LayoutSize::new(width, height);
        b.stretched_height = Some(height);
    }
    let (is_flex, is_grid, parent_height) = {
        let b = item.borrow();
        (
            b.style().display() == DisplayType::Flex,
            b.style().display() == DisplayType::Grid,
            b.size().height(),
        )
    };
    if is_flex {
        place_items(item, parent_height);
    } else if is_grid {
        crate::renderer::layout::grid::place_items(item, parent_height);
    }
}

fn is_auto_height(item: &Node) -> bool {
    let b = item.borrow();
    let style = b.style();
    !style.has_author_height() && !b.is_replaced_box()
}

fn place_row(container: &Node, parent_height: i64) {
    let (lines, style, width) = {
        let b = container.borrow();
        (b.flex_lines.clone(), b.style(), b.content_size().width())
    };
    let (row_gap, column_gap) = (style.row_gap(), style.column_gap());
    let align = style.align_items();

    // Each line is as tall as its tallest item.
    let outer_height = |item: &Node| {
        let b = item.borrow();
        let m = compute_box_model_metrics(&b.style());
        b.size().height() + m.margin.top + m.margin.bottom
    };
    let mut cross: Vec<i64> = lines
        .iter()
        .map(|line| line.iter().map(outer_height).max().unwrap_or(0))
        .collect();
    let total: i64 = cross.iter().sum::<i64>() + row_gap * (cross.len() as i64 - 1).max(0);
    let (height, definite) = settle_height(container, parent_height, total);

    let free_cross = height - total;
    let mut lead = 0;
    let mut between = 0;
    if definite && !cross.is_empty() {
        if style.flex_wrap() == FlexWrap::NoWrap {
            // A single nowrap line is exactly as tall as the container.
            cross[0] = height;
        } else if style.align_content() == AlignContent::Stretch && free_cross > 0 {
            let share = free_cross / cross.len() as i64;
            for line in cross.iter_mut() {
                *line += share;
            }
        } else {
            let (l, b) = align_content_distribution(
                style.align_content(),
                free_cross,
                cross.len() as i64,
            );
            lead = l;
            between = b;
        }
    }

    let reverse_lines = style.flex_wrap() == FlexWrap::WrapReverse;
    let mut y = lead;
    let count = lines.len();
    for index in 0..count {
        let at = if reverse_lines { count - 1 - index } else { index };
        let line = &lines[at];
        let line_cross = cross[at];
        let used: i64 = line
            .iter()
            .map(|item| {
                let b = item.borrow();
                let m = compute_box_model_metrics(&b.style());
                b.size().width() + m.margin.left + m.margin.right
            })
            .sum();
        let gaps = column_gap * (line.len() as i64 - 1).max(0);
        let free = width - used - gaps;
        let auto_margins: i64 = line
            .iter()
            .map(|item| {
                let s = item.borrow().style();
                s.margin_left_auto() as i64 + s.margin_right_auto() as i64
            })
            .sum();
        let auto_share = if auto_margins > 0 { free.max(0) / auto_margins } else { 0 };
        let (main_lead, main_between) = if auto_margins > 0 {
            (0, 0)
        } else {
            distribute(style.justify_content(), free, line.len() as i64)
        };
        let mut x = main_lead;
        for item in line {
            let (size, metrics, item_style) = {
                let b = item.borrow();
                (b.size(), compute_box_model_metrics(&b.style()), b.style())
            };
            if item_style.margin_left_auto() {
                x += auto_share;
            }
            let outer_w = size.width() + metrics.margin.left + metrics.margin.right;
            let item_x = if style.flex_reverse() {
                width - (x + outer_w) + metrics.margin.left
            } else {
                x + metrics.margin.left
            };

            let cross_auto = item_style.margin_top_auto() || item_style.margin_bottom_auto();
            let self_align = item_style.align_self_or(align);
            let room = line_cross - metrics.margin.top - metrics.margin.bottom;
            let mut item_y = y + metrics.margin.top;
            if cross_auto {
                let spare = (room - size.height()).max(0);
                let parts =
                    item_style.margin_top_auto() as i64 + item_style.margin_bottom_auto() as i64;
                if item_style.margin_top_auto() {
                    item_y += spare / parts;
                }
            } else {
                match self_align {
                    AlignItems::Stretch => {
                        if is_auto_height(item) && room != size.height() {
                            let stretched = room.max(0);
                            let limited = item_style.clamp_height(stretched, height);
                            relayout_after_stretch(item, limited);
                        }
                    }
                    AlignItems::Center => item_y += (room - size.height()) / 2,
                    AlignItems::FlexEnd => item_y += room - size.height(),
                    AlignItems::FlexStart | AlignItems::Baseline => {}
                }
            }
            item.borrow_mut().inline_offset = Some((item_x, item_y));

            x += outer_w + column_gap + main_between;
            if item_style.margin_right_auto() {
                x += auto_share;
            }
        }
        y += line_cross + row_gap + between;
    }
    apply_baselines(&lines, &cross, align);
}

/// `align-items: baseline`: line up each line's baseline-aligned items.
fn apply_baselines(lines: &[Vec<Node>], _cross: &[i64], align: AlignItems) {
    for line in lines {
        let aligned: Vec<&Node> = line
            .iter()
            .filter(|item| item.borrow().style().align_self_or(align) == AlignItems::Baseline)
            .collect();
        if aligned.len() < 2 {
            continue;
        }
        let baseline_of = |item: &Node| -> i64 {
            let b = item.borrow();
            let m = compute_box_model_metrics(&b.style());
            m.margin.top + b.first_baseline_for_flex()
        };
        let deepest = aligned.iter().map(|i| baseline_of(i)).max().unwrap_or(0);
        for item in aligned {
            let shift = deepest - baseline_of(item);
            let mut b = item.borrow_mut();
            if let Some((x, y)) = b.inline_offset {
                b.inline_offset = Some((x, y + shift));
            }
        }
    }
}

fn place_column(container: &Node, parent_height: i64) {
    let (lines, style, width) = {
        let b = container.borrow();
        (b.flex_lines.clone(), b.style(), b.content_size().width())
    };
    let nodes: Vec<Node> = lines.into_iter().flatten().collect();
    let gap = style.row_gap();
    let gaps = gap * (nodes.len() as i64 - 1).max(0);

    let mut items: Vec<FlexItem> = nodes
        .iter()
        .map(|node| {
            let b = node.borrow();
            let item_style = b.style();
            let m = compute_box_model_metrics(&item_style);
            let content = (b.size().height() - m.inner_vertical()).max(0) as f64;
            let declared_basis = item_style.flex_basis().filter(|basis| *basis > 0.0);
            let min = if item_style.overflow_clip() || item_style.overflow_scrollable() {
                0.0
            } else {
                content
            };
            FlexItem {
                node: node.clone(),
                base: declared_basis.unwrap_or(content),
                outer_extra: (m.inner_vertical() + m.margin.top + m.margin.bottom) as f64,
                min: min.min(declared_basis.unwrap_or(f64::INFINITY)),
                max: f64::INFINITY,
                grow: item_style.flex_grow(),
                shrink: item_style.flex_shrink(),
                target: content,
                frozen: false,
            }
        })
        .collect();

    let natural: i64 = items
        .iter()
        .map(|i| i.hypothetical_outer().ceil() as i64)
        .sum::<i64>()
        + gaps;
    let (height, definite) = settle_height(container, parent_height, natural);
    if definite {
        resolve_lengths(&mut items, height as f64, gaps as f64);
    } else {
        for item in &mut items {
            item.target = item.hypothetical();
        }
    }

    let used: i64 = items
        .iter()
        .map(|i| (i.target + i.outer_extra).round() as i64)
        .sum::<i64>()
        + gaps;
    let free = height - used;
    let auto_margins: i64 = nodes
        .iter()
        .map(|n| {
            let s = n.borrow().style();
            s.margin_top_auto() as i64 + s.margin_bottom_auto() as i64
        })
        .sum();
    let auto_share = if auto_margins > 0 { free.max(0) / auto_margins } else { 0 };
    let (lead, between) = if auto_margins > 0 {
        (0, 0)
    } else {
        distribute(style.justify_content(), free, nodes.len() as i64)
    };
    let align = style.align_items();
    let mut y = lead;
    for item in &items {
        let (metrics, item_style, current) = {
            let b = item.node.borrow();
            (compute_box_model_metrics(&b.style()), b.style(), b.size())
        };
        let outer_height = (item.target + item.outer_extra).round() as i64;
        let border_box_height =
            outer_height - metrics.margin.top - metrics.margin.bottom;
        if border_box_height != current.height() {
            relayout_after_stretch(&item.node, border_box_height);
        }
        if item_style.margin_top_auto() {
            y += auto_share;
        }
        let room = width - metrics.margin.left - metrics.margin.right;
        let item_w = item.node.borrow().size().width();
        let item_x = if item_style.margin_left_auto() && item_style.margin_right_auto() {
            metrics.margin.left + (room - item_w) / 2
        } else if item_style.margin_left_auto() {
            metrics.margin.left + room - item_w
        } else {
            match item_style.align_self_or(align) {
                AlignItems::Center => metrics.margin.left + (room - item_w) / 2,
                AlignItems::FlexEnd => metrics.margin.left + room - item_w,
                _ => metrics.margin.left,
            }
        };
        let item_y = if style.flex_reverse() {
            height - y - outer_height + metrics.margin.top
        } else {
            y + metrics.margin.top
        };
        item.node.borrow_mut().inline_offset = Some((item_x, item_y));
        y += outer_height + gap + between;
        if item_style.margin_bottom_auto() {
            y += auto_share;
        }
    }
}
