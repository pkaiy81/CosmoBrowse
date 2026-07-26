//! `@keyframes` animation playback (Phase 4.4).
//!
//! Where the transition driver reacts to a target *changing*, this one plays a
//! declared timeline: for each element running an `animation`, it tracks when
//! playback started and, every frame, works out the current iteration and the
//! progress within it, finds the surrounding keyframes, and interpolates.
//!
//! The interpolated value is written to the same `data-cosmo-anim-*` attributes
//! the transition driver uses, so painting, layout and the
//! `COSMO_LAYOUT_ASSERT` safety net need no knowledge of which driver produced
//! a frame. Timelines arrive already resolved to concrete values per element
//! (see `LayoutView::collect_animation_targets`).
//!
//! Spec: CSS Animations Level 1 — https://www.w3.org/TR/css-animations-1/

use cosmo_engine::renderer::dom::node::Node;
use cosmo_engine::renderer::layout::computed_style::{
    AnimatedProperty, AnimatedValue, AnimationSpec,
};
use cosmo_engine::renderer::layout::layout_view::AnimationTarget;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use super::transitions::{clear_override, node_key, write_override};

/// One element's running animation.
struct ActiveAnimation {
    node: Rc<RefCell<Node>>,
    spec: AnimationSpec,
    timelines: Vec<(AnimatedProperty, Vec<(f64, AnimatedValue)>)>,
    /// Clock reading when playback began (before `animation-delay`).
    start_ms: f64,
    /// True once the animation has run out of iterations; kept in the map only
    /// so a `forwards` fill keeps its final value applied.
    finished: bool,
}

impl ActiveAnimation {
    /// The value of each animated property at `now`, or `None` when nothing
    /// should be applied (before a non-backwards-filling animation starts, or
    /// after a non-forwards-filling one ends).
    fn values_at(&self, now: f64) -> Option<Vec<(AnimatedProperty, AnimatedValue)>> {
        let duration = self.spec.duration_ms.max(1) as f64;
        let elapsed = now - self.start_ms - self.spec.delay_ms as f64;

        // Still delayed: only a `backwards` fill shows anything yet.
        if elapsed < 0.0 {
            if !self.spec.fill_mode.fills_backwards() {
                return None;
            }
            return Some(self.sample(0, 0.0));
        }

        let total_iterations = self.spec.iterations;
        let raw_iteration = elapsed / duration;
        let done = total_iterations.is_some_and(|count| raw_iteration >= count);
        if done {
            if !self.spec.fill_mode.fills_forwards() {
                return None;
            }
            // Hold the end of the last iteration that ran.
            let count = total_iterations.expect("checked by is_some_and");
            let last = (count.ceil() as u32).saturating_sub(1);
            let end = if count.fract() == 0.0 { 1.0 } else { count.fract() };
            return Some(self.sample(last, end));
        }
        Some(self.sample(raw_iteration as u32, raw_iteration.fract()))
    }

    /// Sample every timeline at `progress` within iteration `index`, honouring
    /// `animation-direction` and the easing.
    fn sample(&self, index: u32, progress: f64) -> Vec<(AnimatedProperty, AnimatedValue)> {
        let progress = if self.spec.direction.reversed(index) {
            1.0 - progress
        } else {
            progress
        };
        let eased = self.spec.easing.apply(progress);
        self.timelines
            .iter()
            .filter_map(|(property, timeline)| {
                sample_timeline(timeline, eased).map(|value| (*property, value))
            })
            .collect()
    }

    /// Whether this animation still needs frames driven for it.
    fn running(&self) -> bool {
        !self.finished
    }
}

/// Interpolate `timeline` (sorted by offset) at `progress`.
fn sample_timeline(timeline: &[(f64, AnimatedValue)], progress: f64) -> Option<AnimatedValue> {
    let first = timeline.first()?;
    let last = timeline.last()?;
    if progress <= first.0 {
        return Some(first.1.clone());
    }
    if progress >= last.0 {
        return Some(last.1.clone());
    }
    // The pair of frames the progress falls between.
    let upper = timeline.iter().position(|(offset, _)| *offset >= progress)?;
    let (from_offset, from_value) = &timeline[upper.saturating_sub(1)];
    let (to_offset, to_value) = &timeline[upper];
    let span = to_offset - from_offset;
    let local = if span > 0.0 {
        (progress - from_offset) / span
    } else {
        1.0
    };
    Some(from_value.lerp(to_value, local))
}

/// Per-page `@keyframes` playback state, keyed by DOM node identity.
#[derive(Default)]
pub(crate) struct KeyframeDriver {
    active: HashMap<usize, ActiveAnimation>,
    clock_ms: f64,
}

impl KeyframeDriver {
    /// Whether any animation still needs frames (a `forwards` fill that has
    /// finished holds its value without asking for more).
    pub fn is_animating(&self) -> bool {
        self.active.values().any(ActiveAnimation::running)
    }

    /// Reconcile the running animations with what the freshly-computed style
    /// declares: start newly-declared ones, drop ones whose element or
    /// declaration went away, and apply the current frame's values. Returns
    /// true if any override changed (the caller re-lays-out).
    pub fn sync(&mut self, targets: &[AnimationTarget]) -> bool {
        let mut changed = false;
        let mut seen: HashSet<usize> = HashSet::with_capacity(targets.len());
        for target in targets {
            let key = node_key(&target.node);
            seen.insert(key);
            match self.active.get_mut(&key) {
                // Re-declaring the same animation must not restart it — the
                // style is recomputed on every layout, including the ones this
                // driver itself triggers.
                Some(active) if active.spec.name == target.spec.name => {
                    active.timelines = target.timelines.clone();
                    active.spec = target.spec.clone();
                }
                _ => {
                    self.active.insert(
                        key,
                        ActiveAnimation {
                            node: target.node.clone(),
                            spec: target.spec.clone(),
                            timelines: target.timelines.clone(),
                            start_ms: self.clock_ms,
                            finished: false,
                        },
                    );
                    changed = true;
                }
            }
        }
        // Elements that stopped animating (removed, or `animation` gone) give
        // their overrides back to the cascade.
        let dropped: Vec<usize> = self
            .active
            .keys()
            .filter(|key| !seen.contains(key))
            .copied()
            .collect();
        for key in dropped {
            if let Some(animation) = self.active.remove(&key) {
                for (property, _) in &animation.timelines {
                    clear_override(&animation.node, *property);
                }
                changed = true;
            }
        }
        changed | self.apply(self.clock_ms)
    }

    /// Advance the clock and re-apply every running animation. Returns true if
    /// any override changed.
    pub fn advance(&mut self, dt_ms: f64) -> bool {
        if self.active.is_empty() {
            return false;
        }
        self.clock_ms += dt_ms;
        self.apply(self.clock_ms)
    }

    /// Write each animation's value at `now`.
    fn apply(&mut self, now: f64) -> bool {
        let mut changed = false;
        for animation in self.active.values_mut() {
            match animation.values_at(now) {
                Some(values) => {
                    for (property, value) in values {
                        if write_override(&animation.node, property, &value) {
                            changed = true;
                        }
                    }
                }
                None => {
                    for (property, _) in &animation.timelines {
                        clear_override(&animation.node, *property);
                    }
                }
            }
            // Once the iterations are used up the animation stops asking for
            // frames; a `forwards` fill keeps its last value applied above.
            let duration = animation.spec.duration_ms.max(1) as f64;
            let elapsed = now - animation.start_ms - animation.spec.delay_ms as f64;
            animation.finished = animation
                .spec
                .iterations
                .is_some_and(|count| elapsed / duration >= count);
        }
        changed
    }
}
