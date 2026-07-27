//! Inline formatting context: line-box construction (Phase 2.5).
//!
//! The engine's existing inline layout wraps each text node independently
//! against the *whole* containing block, which is wrong as soon as a line holds
//! more than one thing: a run that starts halfway along a line still gets the
//! full block width to wrap in, so it overflows the right edge, and nothing
//! shortens a line for a float.
//!
//! This module models the real thing — a sequence of inline-level items packed
//! into line boxes — with two properties the old path can't express:
//!
//! * a line's available width comes from the [`FloatContext`] at that Y, so
//!   floats shorten individual lines (Phase 2.3's other half);
//! * breaking is decided across item boundaries, so a run continues on the line
//!   its predecessor left off and breaks against the room actually left.
//!
//! It is deliberately pure: items in, positioned fragments out, no layout-tree
//! access. That is what lets it be unit-tested to the level this replacement
//! needs before any of it is wired into the layout passes.
//!
//! Spec: CSS 2.2 §9.4.2 (inline formatting contexts) and §10.8 (line height /
//! baseline alignment). https://www.w3.org/TR/CSS22/visuren.html#inline-formatting

use crate::renderer::layout::computed_style::FontSize;
use crate::renderer::layout::floats::FloatContext;
use crate::renderer::text::legacy_metrics::{char_advance, is_break_space, measure_text_width};

/// A run of text with the font it is measured in.
#[derive(Debug, Clone, PartialEq)]
pub struct TextRun {
    pub text: String,
    pub font_size: FontSize,
    pub bold: bool,
    /// Used line-height: the height this run contributes to a line box.
    pub line_height: i64,
}

/// One inline-level thing to place on a line.
#[derive(Debug, Clone, PartialEq)]
pub enum InlineItem {
    /// Breakable text.
    Text(TextRun),
    /// An atomic inline (inline-block, image, replaced element): placed whole,
    /// moved to the next line if it doesn't fit.
    Atomic {
        width: i64,
        height: i64,
        /// Distance from the box's top to its baseline.
        baseline: i64,
    },
}

impl InlineItem {
    /// Where this item's baseline sits below its own top edge — public so the
    /// layout tree can shift a fragment onto its line's shared baseline.
    pub fn baseline_offset(&self) -> i64 {
        self.baseline()
    }

    fn height(&self) -> i64 {
        match self {
            Self::Text(run) => run.line_height,
            Self::Atomic { height, .. } => *height,
        }
    }

    /// Where this item's baseline sits below its own top edge.
    ///
    /// The engine paints a text run from its box top and treats the baseline as
    /// one font-size below that — the convention `align_inline_baselines` uses.
    /// Line boxes must agree with it, or runs of different sizes on one line end
    /// up a pixel apart from where the rest of the engine expects them.
    fn baseline(&self) -> i64 {
        match self {
            Self::Text(run) => run.font_size.px(),
            Self::Atomic { baseline, .. } => *baseline,
        }
    }
}

/// A piece of one item placed on a line. A text item breaking over three lines
/// yields three fragments.
#[derive(Debug, Clone, PartialEq)]
pub struct Fragment {
    /// Index into the item list this fragment came from.
    pub item: usize,
    /// The text actually on this line (`None` for atomic items).
    pub text: Option<String>,
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

/// One line box: the fragments on it, and the line's own geometry.
#[derive(Debug, Clone, PartialEq)]
pub struct LineBox {
    pub y: i64,
    pub height: i64,
    /// Baseline offset from the line's top edge.
    pub baseline: i64,
    pub fragments: Vec<Fragment>,
}

impl LineBox {
    /// The x just past the last fragment — where a following item continues.
    pub fn end_x(&self) -> i64 {
        self.fragments
            .last()
            .map(|f| f.x + f.width)
            .unwrap_or(0)
    }
}

/// How the line's leftover space is distributed.
/// Spec: CSS 2.2 §16.2 — `text-align`. https://www.w3.org/TR/CSS22/text.html#alignment
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineAlign {
    Start,
    Center,
    End,
}

/// Lays `items` out into line boxes starting at `start_y`, inside a content box
/// `content_width` wide, with each line's usable span taken from `floats`.
///
/// `start_x` is where the first line begins (a block whose inline content
/// continues after something already placed).
pub fn layout_inline_items(
    items: &[InlineItem],
    floats: &FloatContext,
    content_width: i64,
    start_y: i64,
    start_x: i64,
) -> Vec<LineBox> {
    layout_inline_items_aligned(items, floats, content_width, start_y, start_x, LineAlign::Start)
}

/// As [`layout_inline_items`], distributing each line's leftover space per
/// `align`.
pub fn layout_inline_items_aligned(
    items: &[InlineItem],
    floats: &FloatContext,
    content_width: i64,
    start_y: i64,
    start_x: i64,
    align: LineAlign,
) -> Vec<LineBox> {
    let items = &collapse_across_items(items);
    let mut lines: Vec<LineBox> = Vec::new();
    // Provisional line height so the float band query has a height to test
    // against before anything is on the line.
    let probe_height = items.first().map(InlineItem::height).unwrap_or(1);
    // The first line begins after any float already occupying the left edge.
    let first_x = start_x.max(band_left(floats, content_width, start_y, probe_height));
    let mut current = OpenLine::new(start_y, first_x);

    for (index, item) in items.iter().enumerate() {
        match item {
            InlineItem::Atomic { width, height, .. } => {
                let band = band_for(floats, content_width, current.y, (*height).max(probe_height));
                if current.needs_break(*width, band.right) {
                    lines.push(current.close());
                    current = OpenLine::new(next_y(&lines), band_left(floats, content_width, next_y(&lines), *height));
                }
                current.push(Fragment {
                    item: index,
                    text: None,
                    x: current.x,
                    y: current.y,
                    width: *width,
                    height: *height,
                }, item);
            }
            InlineItem::Text(run) => {
                // Break opportunities for the whole run, computed once: they
                // are a property of the text, not of the line it lands on, and
                // recomputing per line would be quadratic on a long run.
                let breaks = break_opportunities(&run.text);
                let mut rest = run.text.as_str();
                loop {
                    let band = band_for(
                        floats,
                        content_width,
                        current.y,
                        run.line_height.max(probe_height),
                    );
                    let available = (band.right - current.x).max(0);
                    let base = run.text.len() - rest.len();
                    let (head, tail) =
                        break_text(rest, base, &breaks, run, available, current.is_empty());
                    if head.is_empty() {
                        // Nothing fits on what's left of this line. If the line
                        // already holds something, move on.
                        if !current.is_empty() {
                            lines.push(current.close());
                            let y = next_y(&lines);
                            current = OpenLine::new(y, band_left(floats, content_width, y, run.line_height));
                            continue;
                        }
                        // The line is empty and still nothing fits: the floats
                        // beside it leave too little room, so the line moves
                        // *down* past them rather than being squeezed to
                        // nothing (CSS 2.2 §9.5 — a line box that cannot fit
                        // next to floats is moved down until it fits).
                        if let Some(below) = floats.next_edge_below(current.y) {
                            current = OpenLine::new(
                                below,
                                band_left(floats, content_width, below, run.line_height),
                            );
                            continue;
                        }
                        // No floats left to clear: take one character so
                        // layout always progresses.
                        let mut chars = rest.char_indices();
                        chars.next();
                        let split = chars.next().map(|(i, _)| i).unwrap_or(rest.len());
                        let (forced, remainder) = rest.split_at(split);
                        current.push(
                            Fragment {
                                item: index,
                                text: Some(forced.to_string()),
                                x: current.x,
                                y: current.y,
                                width: measure_text_width(forced, run.font_size, run.bold),
                                height: run.line_height,
                            },
                            item,
                        );
                        rest = remainder;
                    } else {
                        // White space at a line break hangs: drop it from the
                        // fragment so it neither widens the line nor shifts an
                        // alignment. A fragment that does NOT end a line keeps
                        // its trailing space — that space separates it from
                        // whatever follows on the same line.
                        let painted = if tail.is_empty() {
                            head
                        } else {
                            head.trim_end_matches(is_break_space)
                        };
                        current.push(
                            Fragment {
                                item: index,
                                text: Some(painted.to_string()),
                                x: current.x,
                                y: current.y,
                                width: measure_text_width(painted, run.font_size, run.bold),
                                height: run.line_height,
                            },
                            item,
                        );
                        rest = tail;
                    }
                    if rest.is_empty() {
                        break;
                    }
                    // More text to place: it goes on a fresh line.
                    lines.push(current.close());
                    let y = next_y(&lines);
                    current = OpenLine::new(y, band_left(floats, content_width, y, run.line_height));
                }
            }
        }
    }
    if !current.is_empty() {
        lines.push(current.close());
    }
    if align != LineAlign::Start {
        for line in &mut lines {
            let band = band_for(floats, content_width, line.y, line.height);
            let used = line.end_x() - line.fragments.first().map(|f| f.x).unwrap_or(0);
            let free = (band.right - band.left - used).max(0);
            let shift = match align {
                LineAlign::Center => free / 2,
                LineAlign::End => free,
                LineAlign::Start => 0,
            };
            for fragment in &mut line.fragments {
                fragment.x += shift;
            }
        }
    }
    lines
}

/// Collapse white space *across* item boundaries: a space that ends one run and
/// starts the next is one space, not two. Each run has already had its own runs
/// of white space collapsed, but nothing could see across the boundary while
/// every text node wrapped independently — which is why an inline element used
/// to be preceded by a visible double space.
/// Spec: CSS Text §4.1.1 — the white-space processing model.
/// https://www.w3.org/TR/css-text-3/#white-space-phase-1
fn collapse_across_items(items: &[InlineItem]) -> Vec<InlineItem> {
    let mut out: Vec<InlineItem> = Vec::with_capacity(items.len());
    let mut previous_ended_with_space = true; // a line's leading space is dropped
    for item in items {
        match item {
            InlineItem::Text(run) => {
                let mut text = run.text.as_str();
                if previous_ended_with_space {
                    text = text.trim_start_matches(is_break_space);
                }
                if text.is_empty() {
                    // Nothing left: the run was pure white space already
                    // represented by the preceding one.
                    continue;
                }
                previous_ended_with_space = text.ends_with(is_break_space);
                out.push(InlineItem::Text(TextRun {
                    text: text.to_string(),
                    ..run.clone()
                }));
            }
            atomic => {
                previous_ended_with_space = false;
                out.push(atomic.clone());
            }
        }
    }
    out
}

/// The usable span at `y` for a line `height` tall.
fn band_for(
    floats: &FloatContext,
    content_width: i64,
    y: i64,
    height: i64,
) -> crate::renderer::layout::floats::Band {
    if floats.is_empty() {
        return crate::renderer::layout::floats::Band {
            left: 0,
            right: content_width,
        };
    }
    floats.band(y, height)
}

fn band_left(floats: &FloatContext, content_width: i64, y: i64, height: i64) -> i64 {
    band_for(floats, content_width, y, height).left
}

/// The Y a new line starts at: just below the last closed one.
fn next_y(lines: &[LineBox]) -> i64 {
    lines.last().map(|l| l.y + l.height).unwrap_or(0)
}

/// The UAX #14 break opportunities in `text`, as byte offsets at which a line
/// may end (the offset is the start of the next line). Computed once per run.
///
/// Spec: UAX #14, Unicode Line Breaking Algorithm. https://www.unicode.org/reports/tr14/
fn break_opportunities(text: &str) -> Vec<usize> {
    unicode_linebreak::linebreaks(text)
        .map(|(offset, _)| offset)
        .collect()
}

/// Split `text` — the untaken remainder of a run starting at byte `base` within
/// it — at the last break opportunity whose content fits in `available`.
/// Returns (what goes on this line, what is left).
///
/// Trailing white space at the break does not count against the width: a line
/// may end with spaces that hang past the edge rather than pushing the next
/// word down (CSS Text §4.1, hanging white space). When `line_empty` and not
/// even the first opportunity fits, the text is hard-broken so layout always
/// progresses.
fn break_text<'a>(
    text: &'a str,
    base: usize,
    breaks: &[usize],
    run: &TextRun,
    available: i64,
    line_empty: bool,
) -> (&'a str, &'a str) {
    // Walk once, recording each opportunity together with the width of the
    // content up to it excluding any trailing spaces.
    let mut width = 0i64;
    let mut width_without_trailing_space = 0i64;
    let mut best: Option<usize> = None;
    let mut next_break = breaks.partition_point(|b| *b <= base);
    let mut hard_break_at: Option<usize> = None;

    for (offset, ch) in text.char_indices() {
        let absolute = base + offset;
        // Opportunities at or before this character are now measurable.
        while next_break < breaks.len() && breaks[next_break] <= absolute {
            if width_without_trailing_space <= available {
                best = Some(breaks[next_break] - base);
            }
            next_break += 1;
        }
        if width_without_trailing_space > available {
            // Nothing further can fit; stop rather than scanning the rest.
            break;
        }
        if hard_break_at.is_none() && width + char_advance(ch, run.font_size, run.bold) > available
        {
            hard_break_at = Some(offset);
        }
        width += char_advance(ch, run.font_size, run.bold);
        if !is_break_space(ch) {
            width_without_trailing_space = width;
        }
    }
    // The end of the run is itself an opportunity.
    while next_break < breaks.len() && breaks[next_break] <= base + text.len() {
        if width_without_trailing_space <= available {
            best = Some(breaks[next_break] - base);
        }
        next_break += 1;
    }

    match best {
        Some(end) => (&text[..end], &text[end..]),
        // No opportunity fits. On an empty line the word is longer than the
        // line, so hard-break it rather than produce nothing.
        None if line_empty => match hard_break_at.filter(|at| *at > 0) {
            Some(at) => (&text[..at], &text[at..]),
            None => ("", text),
        },
        None => ("", text),
    }
}

/// A line being filled.
struct OpenLine {
    y: i64,
    x: i64,
    start_x: i64,
    height: i64,
    baseline: i64,
    fragments: Vec<Fragment>,
}

impl OpenLine {
    fn new(y: i64, start_x: i64) -> Self {
        Self {
            y,
            x: start_x,
            start_x,
            height: 0,
            baseline: 0,
            fragments: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.fragments.is_empty()
    }

    /// Whether adding `width` would overflow a line ending at `right`.
    fn needs_break(&self, width: i64, right: i64) -> bool {
        !self.is_empty() && self.x + width > right
    }

    fn push(&mut self, fragment: Fragment, item: &InlineItem) {
        self.x = fragment.x + fragment.width;
        // The line grows to fit the tallest item, aligned on the deepest
        // baseline (CSS 2.2 §10.8).
        self.baseline = self.baseline.max(item.baseline());
        let below = item.height() - item.baseline();
        self.height = self.height.max(self.baseline + below);
        self.fragments.push(fragment);
    }

    /// Finish the line: shift every fragment so their baselines coincide.
    fn close(mut self) -> LineBox {
        let baseline = self.baseline;
        for fragment in &mut self.fragments {
            fragment.y = self.y;
        }
        LineBox {
            y: self.y,
            height: self.height.max(1),
            baseline,
            fragments: core::mem::take(&mut self.fragments),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::layout::computed_style::{Clear, FontSize};
    use crate::renderer::layout::floats::FloatSide;

    fn text(s: &str) -> InlineItem {
        InlineItem::Text(TextRun {
            text: s.to_string(),
            font_size: FontSize::Medium,
            bold: false,
            line_height: 20,
        })
    }

    fn width_of(s: &str) -> i64 {
        measure_text_width(s, FontSize::Medium, false)
    }

    fn lines_text(lines: &[LineBox]) -> Vec<Vec<String>> {
        lines
            .iter()
            .map(|line| {
                line.fragments
                    .iter()
                    .filter_map(|f| f.text.clone())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_single_run_fitting_on_one_line_is_one_fragment() {
        let items = vec![text("hello world")];
        let lines = layout_inline_items(&items, &FloatContext::default(), 10_000, 0, 0);
        assert_eq!(lines_text(&lines), vec![vec!["hello world".to_string()]]);
        assert_eq!(lines[0].height, 20);
    }

    #[test]
    fn a_run_breaks_at_spaces_when_it_overflows() {
        // Room for "aaa bbb" but not the third word.
        let available = width_of("aaa bbb ");
        let items = vec![text("aaa bbb ccc")];
        let lines = layout_inline_items(&items, &FloatContext::default(), available, 0, 0);
        assert_eq!(
            lines_text(&lines),
            vec![vec!["aaa bbb".to_string()], vec!["ccc".to_string()]]
        );
        // The second line starts below the first.
        assert_eq!(lines[1].y, lines[0].y + lines[0].height);
    }

    #[test]
    fn a_run_continues_on_the_line_the_previous_item_left_off() {
        // This is what the per-node path cannot do: the second run must wrap
        // against the room *left* on the line, not the whole block width.
        let content = width_of("aaa bbb");
        let items = vec![text("aaa "), text("bbb ccc")];
        let lines = layout_inline_items(&items, &FloatContext::default(), content, 0, 0);
        assert_eq!(
            lines_text(&lines),
            vec![
                vec!["aaa ".to_string(), "bbb".to_string()],
                vec!["ccc".to_string()]
            ],
            "the second run continues the first line, then wraps"
        );
        // The continuing fragment starts where the first ended.
        assert_eq!(lines[0].fragments[1].x, lines[0].fragments[0].width);
    }

    #[test]
    fn a_word_too_long_for_the_line_is_hard_broken() {
        let items = vec![text("abcdefgh")];
        let narrow = width_of("abc");
        let lines = layout_inline_items(&items, &FloatContext::default(), narrow, 0, 0);
        assert!(lines.len() > 1, "must break rather than overflow");
        let rejoined: String = lines_text(&lines)
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .concat();
        assert_eq!(rejoined, "abcdefgh", "no characters lost");
    }

    #[test]
    fn an_atomic_item_that_does_not_fit_moves_to_the_next_line() {
        let content = width_of("aaa") + 10;
        let items = vec![
            text("aaa"),
            InlineItem::Atomic {
                width: content,
                height: 30,
                baseline: 30,
            },
        ];
        let lines = layout_inline_items(&items, &FloatContext::default(), content, 0, 0);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1].fragments[0].item, 1);
        assert_eq!(lines[1].height, 30, "the line grows to the atomic's height");
    }

    #[test]
    fn a_float_shortens_the_lines_it_spans() {
        // A left float 40 wide and 20 tall: the first line starts after it,
        // later lines get the full width back.
        let content = width_of("aaa bbb ccc ddd");
        let mut floats = FloatContext::new(content);
        floats.place(FloatSide::Left, 40, 20, 0, Clear::None);
        let items = vec![text("aaa bbb ccc ddd")];
        let lines = layout_inline_items(&items, &floats, content, 0, 40);

        assert!(lines.len() > 1, "the shortened first line must wrap");
        assert_eq!(lines[0].fragments[0].x, 40, "the first line clears the float");
        // The float is 20 tall and lines are 20 tall, so the second line is
        // already past it and starts at the content edge.
        assert_eq!(lines[1].fragments[0].x, 0);
    }

    #[test]
    fn baselines_align_items_of_different_heights() {
        let items = vec![
            text("x"),
            InlineItem::Atomic {
                width: 10,
                height: 40,
                baseline: 40,
            },
        ];
        let lines = layout_inline_items(&items, &FloatContext::default(), 10_000, 0, 0);
        assert_eq!(lines.len(), 1);
        // The tall atomic sets the baseline; the line is at least that tall.
        assert_eq!(lines[0].baseline, 40);
        assert!(lines[0].height >= 40);
    }

    #[test]
    fn uax14_keeps_punctuation_off_the_start_of_a_line() {
        // A Japanese full stop may not begin a line (UAX #14 class CL): the
        // break has to happen before the character it follows instead. The old
        // space-only rule broke anywhere and stranded it.
        let items = vec![InlineItem::Text(TextRun {
            text: "本日は晴天なり。".to_string(),
            font_size: FontSize::Medium,
            bold: false,
            line_height: 20,
        })];
        // Room for roughly seven of the eight double-width characters.
        let narrow = measure_text_width("本日は晴天なり", FontSize::Medium, false);
        let lines = layout_inline_items(&items, &FloatContext::default(), narrow, 0, 0);
        let first: String = lines[0]
            .fragments
            .iter()
            .filter_map(|f| f.text.clone())
            .collect();
        assert!(
            !lines[1..]
                .iter()
                .flat_map(|l| l.fragments.iter())
                .filter_map(|f| f.text.as_deref())
                .any(|t| t.starts_with('。')),
            "a full stop must not start a line, got {:?}",
            lines_text(&lines)
        );
        assert!(first.ends_with('。') || first.len() < "本日は晴天なり。".len());
    }

    #[test]
    fn uax14_does_not_break_inside_a_word_with_an_apostrophe() {
        // "don't" is one word: the only break opportunities are around it.
        let items = vec![text("well don't stop")];
        let available = width_of("well don't");
        let lines = layout_inline_items(&items, &FloatContext::default(), available, 0, 0);
        assert_eq!(
            lines_text(&lines),
            vec![vec!["well don't".to_string()], vec!["stop".to_string()]]
        );
    }

    #[test]
    fn uax14_breaks_after_a_hyphen() {
        // A hyphen offers a break after it, which the space-only rule missed.
        let items = vec![text("state-of-the-art design")];
        let available = width_of("state-of-");
        let lines = layout_inline_items(&items, &FloatContext::default(), available, 0, 0);
        assert!(lines.len() > 1);
        assert!(
            lines[0]
                .fragments
                .iter()
                .filter_map(|f| f.text.as_deref())
                .any(|t| t.ends_with('-')),
            "expected the first line to end at a hyphen, got {:?}",
            lines_text(&lines)
        );
    }

    #[test]
    fn empty_input_produces_no_lines() {
        let lines = layout_inline_items(&[], &FloatContext::default(), 100, 0, 0);
        assert!(lines.is_empty());
    }
}
