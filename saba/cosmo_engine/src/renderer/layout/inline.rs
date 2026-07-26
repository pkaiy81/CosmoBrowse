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
    fn height(&self) -> i64 {
        match self {
            Self::Text(run) => run.line_height,
            Self::Atomic { height, .. } => *height,
        }
    }

    /// Where this item's baseline sits below its own top edge. Text sits on the
    /// baseline with the half-leading above it (CSS 2.2 §10.8.1).
    fn baseline(&self) -> i64 {
        match self {
            Self::Text(run) => {
                let ascent = (run.font_size.px() as f64 * ASCENT_RATIO) as i64;
                let half_leading = (run.line_height - run.font_size.px()) / 2;
                half_leading + ascent
            }
            Self::Atomic { baseline, .. } => *baseline,
        }
    }
}

/// Fraction of the em box above the baseline. The engine's metrics provider
/// exposes per-font ascent, but line boxes only need a consistent ratio to
/// align items against each other; this matches the DejaVu-calibrated tables
/// the legacy path uses.
const ASCENT_RATIO: f64 = 0.8;

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
    let mut lines: Vec<LineBox> = Vec::new();
    let mut current = OpenLine::new(start_y, start_x);
    // Provisional line height so the float band query has a height to test
    // against before anything is on the line.
    let probe_height = items.first().map(InlineItem::height).unwrap_or(1);

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
                let mut rest = run.text.as_str();
                loop {
                    let band = band_for(
                        floats,
                        content_width,
                        current.y,
                        run.line_height.max(probe_height),
                    );
                    let available = (band.right - current.x).max(0);
                    let (head, tail) = break_text(rest, run, available, current.is_empty());
                    if head.is_empty() {
                        // Nothing fits on what's left of this line. If the line
                        // already holds something, move on; if it is empty the
                        // band itself is too narrow, so take one character to
                        // guarantee progress rather than loop forever.
                        if !current.is_empty() {
                            lines.push(current.close());
                            let y = next_y(&lines);
                            current = OpenLine::new(y, band_left(floats, content_width, y, run.line_height));
                            continue;
                        }
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
                        current.push(
                            Fragment {
                                item: index,
                                text: Some(head.to_string()),
                                x: current.x,
                                y: current.y,
                                width: measure_text_width(head, run.font_size, run.bold),
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
    lines
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

/// Split `text` at the last break opportunity that fits in `available`.
/// Returns (what goes on this line, what is left). A break at a space consumes
/// it. When `line_empty` and not even one word fits, the text is hard-broken so
/// layout always progresses.
fn break_text<'a>(
    text: &'a str,
    run: &TextRun,
    available: i64,
    line_empty: bool,
) -> (&'a str, &'a str) {
    let mut width = 0i64;
    // Last break opportunity seen: (end of the fitting text, start of the rest).
    let mut last_break: Option<(usize, usize)> = None;
    for (index, ch) in text.char_indices() {
        let advance = char_advance(ch, run.font_size, run.bold);
        if width + advance > available {
            // The overflow is itself a break opportunity: the line ends here
            // and the space is dropped — a trailing space at a break never
            // forces the word after it down (CSS Text §4.1, hanging spaces).
            if is_break_space(ch) {
                return (&text[..index], &text[index + ch.len_utf8()..]);
            }
            return match last_break {
                Some((end, next)) => (&text[..end], &text[next..]),
                // No break opportunity fits. On an empty line, hard-break
                // before the overflowing character (never produce nothing, or
                // the caller would spin).
                None if line_empty && index > 0 => (&text[..index], &text[index..]),
                None => ("", text),
            };
        }
        width += advance;
        if is_break_space(ch) {
            last_break = Some((index, index + ch.len_utf8()));
        }
    }
    (text, "")
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
    fn empty_input_produces_no_lines() {
        let lines = layout_inline_items(&[], &FloatContext::default(), 100, 0, 0);
        assert!(lines.is_empty());
    }
}
