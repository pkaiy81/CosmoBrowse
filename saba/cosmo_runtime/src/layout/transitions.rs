//! Declarative CSS transition driver (Phase 4.4).
//!
//! The engine's cascade computes the *target* value of a transitioned property;
//! this driver notices when that target moves between layouts, interpolates
//! from the currently-displayed value, and writes the in-between value back
//! onto the DOM node as `data-cosmo-anim-opacity`. Style resolution picks the
//! attribute up as the *used* value (`ComputedStyle::used_opacity`), so the
//! declared value stays intact as the target and a full from-scratch layout
//! reproduces the animated frame exactly (`COSMO_LAYOUT_ASSERT` keeps holding).
//!
//! Spec: CSS Transitions Level 1 — https://www.w3.org/TR/css-transitions-1/
//! Only `opacity` is driven today; colors/transforms interpolate the same way
//! and slot in beside it.

use cosmo_engine::renderer::dom::node::{Node, NodeKind};
use cosmo_engine::renderer::layout::computed_style::{Easing, ANIM_OPACITY_ATTR};
use cosmo_engine::renderer::layout::layout_view::TransitionTarget;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

/// Targets closer than this count as unchanged (and interpolated values closer
/// than this are not worth a repaint).
const EPSILON: f64 = 0.0005;

/// A transition in flight on one element.
struct ActiveTransition {
    node: Rc<RefCell<Node>>,
    from: f64,
    to: f64,
    /// Clock time at which interpolation begins (start + `transition-delay`).
    start_ms: f64,
    duration_ms: f64,
    easing: Easing,
}

impl ActiveTransition {
    /// The interpolated value at `now` (the start value while still delayed).
    fn value_at(&self, now: f64) -> f64 {
        if now <= self.start_ms {
            return self.from;
        }
        let progress = ((now - self.start_ms) / self.duration_ms).clamp(0.0, 1.0);
        self.from + (self.to - self.from) * self.easing.apply(progress)
    }

    fn finished(&self, now: f64) -> bool {
        now >= self.start_ms + self.duration_ms
    }
}

/// Per-page transition state. Keyed by DOM node identity (`Rc::as_ptr`), which
/// is stable because the retained DOM outlives every layout pass.
#[derive(Default)]
pub(crate) struct TransitionDriver {
    active: HashMap<usize, ActiveTransition>,
    /// Target value seen at the previous layout, per element.
    last_target: HashMap<usize, f64>,
    /// Virtual clock, advanced one frame at a time by [`Self::advance`]. Shared
    /// pacing with the script host's frame clock keeps headless runs
    /// deterministic.
    clock_ms: f64,
}

impl TransitionDriver {
    /// Whether a transition is running (the GUI must keep the frame clock alive).
    pub fn is_animating(&self) -> bool {
        !self.active.is_empty()
    }

    /// Compare the freshly-computed targets against the previous layout's and
    /// start transitions where they moved. Returns true if any override was
    /// written, i.e. the caller must re-lay-out so the frame paints the start
    /// value instead of flashing the end state.
    pub fn sync_targets(&mut self, targets: &[TransitionTarget]) -> bool {
        let mut changed = false;
        let mut seen: HashSet<usize> = HashSet::with_capacity(targets.len());
        for target in targets {
            let key = node_key(&target.node);
            seen.insert(key);
            let previous = match self.last_target.insert(key, target.opacity) {
                // First sighting: record the baseline, don't animate. (An
                // element's initial style is not a transition — CSS Transitions
                // §2: only a *change* of computed value starts one.)
                None => continue,
                Some(previous) => previous,
            };
            if (previous - target.opacity).abs() <= EPSILON {
                continue;
            }
            let duration_ms = target.spec.duration_ms as f64;
            if duration_ms <= 0.0 {
                // `transition-duration: 0s` — the change applies instantly and
                // cancels anything in flight.
                if let Some(cancelled) = self.active.remove(&key) {
                    clear_override(&cancelled.node);
                    changed = true;
                }
                continue;
            }
            // Reversing mid-flight starts from the currently displayed value,
            // not from the old target (CSS Transitions §3, transition reversing).
            let from = self
                .active
                .get(&key)
                .map(|a| a.value_at(self.clock_ms))
                .unwrap_or(previous);
            write_override(&target.node, from);
            self.active.insert(
                key,
                ActiveTransition {
                    node: target.node.clone(),
                    from,
                    to: target.opacity,
                    start_ms: self.clock_ms + target.spec.delay_ms as f64,
                    duration_ms,
                    easing: target.spec.easing,
                },
            );
            changed = true;
        }
        // Elements that no longer generate a box (removed, display:none, or
        // their `transition` declaration went away) drop their state; any
        // override they still carry is cleared so the cascade rules again.
        self.last_target.retain(|key, _| seen.contains(key));
        let dropped: Vec<usize> = self
            .active
            .keys()
            .filter(|key| !seen.contains(key))
            .copied()
            .collect();
        for key in dropped {
            if let Some(a) = self.active.remove(&key) {
                clear_override(&a.node);
                changed = true;
            }
        }
        changed
    }

    /// Advance the clock by `dt_ms` and write each running transition's new
    /// value. Returns true if anything moved (the caller re-lays-out/repaints).
    pub fn advance(&mut self, dt_ms: f64) -> bool {
        if self.active.is_empty() {
            return false;
        }
        self.clock_ms += dt_ms;
        let now = self.clock_ms;
        let mut changed = false;
        let mut finished = Vec::new();
        for (key, transition) in &self.active {
            if write_override(&transition.node, transition.value_at(now)) {
                changed = true;
            }
            if transition.finished(now) {
                finished.push(*key);
            }
        }
        for key in finished {
            if let Some(transition) = self.active.remove(&key) {
                // The final value equals the cascade target, so dropping the
                // override is visually a no-op — it just hands control back.
                clear_override(&transition.node);
            }
        }
        changed
    }
}

fn node_key(node: &Rc<RefCell<Node>>) -> usize {
    Rc::as_ptr(node) as *const () as usize
}

/// Write the interpolated value to the node. Returns whether it actually
/// changed (so an unmoved frame costs no re-layout).
fn write_override(node: &Rc<RefCell<Node>>, value: f64) -> bool {
    let text = format!("{:.4}", value.clamp(0.0, 1.0));
    let mut borrowed = node.borrow_mut();
    if let NodeKind::Element(element) = borrowed.kind_mut() {
        if element.get_attribute(ANIM_OPACITY_ATTR).as_deref() == Some(text.as_str()) {
            return false;
        }
        element.set_attribute(ANIM_OPACITY_ATTR, &text);
        return true;
    }
    false
}

fn clear_override(node: &Rc<RefCell<Node>>) {
    let mut borrowed = node.borrow_mut();
    if let NodeKind::Element(element) = borrowed.kind_mut() {
        element.remove_attribute(ANIM_OPACITY_ATTR);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmo_engine::renderer::dom::node::Element;
    use cosmo_engine::renderer::layout::computed_style::TransitionSpec;

    fn element() -> Rc<RefCell<Node>> {
        Rc::new(RefCell::new(Node::new(NodeKind::Element(Element::new(
            "div",
            Vec::new(),
        )))))
    }

    fn target(node: &Rc<RefCell<Node>>, opacity: f64, duration_ms: u32) -> TransitionTarget {
        TransitionTarget {
            node: node.clone(),
            opacity,
            spec: TransitionSpec {
                property: "opacity".to_string(),
                duration_ms,
                delay_ms: 0,
                easing: Easing::Linear,
            },
        }
    }

    fn override_value(node: &Rc<RefCell<Node>>) -> Option<f64> {
        match node.borrow().kind() {
            NodeKind::Element(e) => e
                .get_attribute(ANIM_OPACITY_ATTR)
                .and_then(|v| v.parse::<f64>().ok()),
            _ => None,
        }
    }

    #[test]
    fn first_sighting_does_not_animate() {
        let node = element();
        let mut driver = TransitionDriver::default();
        assert!(!driver.sync_targets(&[target(&node, 1.0, 100)]));
        assert!(!driver.is_animating());
        assert_eq!(override_value(&node), None);
    }

    #[test]
    fn target_change_interpolates_to_completion() {
        let node = element();
        let mut driver = TransitionDriver::default();
        driver.sync_targets(&[target(&node, 1.0, 100)]);

        // The target moves 1.0 -> 0.0 over 100ms: the frame that starts the
        // transition paints the old value, not the new target.
        assert!(driver.sync_targets(&[target(&node, 0.0, 100)]));
        assert!(driver.is_animating());
        assert_eq!(override_value(&node), Some(1.0));

        // Halfway (linear easing) lands on 0.5, still animating.
        assert!(driver.advance(50.0));
        assert_eq!(override_value(&node), Some(0.5));
        assert!(driver.is_animating());

        // At the end the override is released back to the cascade.
        assert!(driver.advance(50.0));
        assert!(!driver.is_animating());
        assert_eq!(override_value(&node), None);
    }

    #[test]
    fn reversal_starts_from_the_displayed_value() {
        let node = element();
        let mut driver = TransitionDriver::default();
        driver.sync_targets(&[target(&node, 1.0, 100)]);
        driver.sync_targets(&[target(&node, 0.0, 100)]);
        driver.advance(25.0);
        assert_eq!(override_value(&node), Some(0.75));

        // Reversing back to 1.0 resumes from 0.75, not from 0.0.
        driver.sync_targets(&[target(&node, 1.0, 100)]);
        assert_eq!(override_value(&node), Some(0.75));
        driver.advance(50.0);
        assert_eq!(override_value(&node), Some(0.875));
    }

    #[test]
    fn zero_duration_applies_instantly() {
        let node = element();
        let mut driver = TransitionDriver::default();
        driver.sync_targets(&[target(&node, 1.0, 0)]);
        assert!(!driver.sync_targets(&[target(&node, 0.0, 0)]));
        assert!(!driver.is_animating());
        assert_eq!(override_value(&node), None);
    }

    #[test]
    fn vanished_element_drops_its_override() {
        let node = element();
        let mut driver = TransitionDriver::default();
        driver.sync_targets(&[target(&node, 1.0, 100)]);
        driver.sync_targets(&[target(&node, 0.0, 100)]);
        assert!(driver.is_animating());

        // The box is gone from the next layout (removed / display:none).
        assert!(driver.sync_targets(&[]));
        assert!(!driver.is_animating());
        assert_eq!(override_value(&node), None);
    }
}
