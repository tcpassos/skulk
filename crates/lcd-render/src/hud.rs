//! On-device HUD/status band: the small strip that composites keyed
//! indicators from many modules (battery, temp, active alert) over whatever
//! screen is in focus. Pure state here — no `embedded-graphics`, same split
//! as [`crate::menu`]; [`crate::Renderer::draw_hud`] does the pixel work.
//!
//! The operator's config fixes which slots show and in what order (a tiny
//! screen can't grow an unbounded band); a module only ever *updates* a slot
//! it doesn't control whether that slot is visible.

use std::collections::HashMap;

use contract::{Severity, WidgetUpdate};

/// The latest state of one visible slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slot {
    pub name: String,
    pub value: String,
    pub severity: Option<Severity>,
}

/// Accumulates [`WidgetUpdate`]s into a fixed, operator-declared set of
/// slots. Updates to slots outside `order` are ignored (the band is bounded
/// on purpose); an update with an empty value clears its slot back to hidden.
#[derive(Debug, Default)]
pub struct Hud {
    /// Declared slot names in display order (left to right).
    order: Vec<String>,
    /// Current value/severity per slot, only for slots that have received a
    /// non-empty update. A slot in `order` but absent here is simply not
    /// drawn yet.
    values: HashMap<String, (String, Option<Severity>)>,
}

impl Hud {
    /// Build a HUD showing `order`'s slots, in that order. Empty `order`
    /// means no band at all.
    pub fn new(order: Vec<String>) -> Self {
        Self { order, values: HashMap::new() }
    }

    /// True when no slot could ever show — lets the renderer skip reserving
    /// any vertical space for the band.
    pub fn is_disabled(&self) -> bool {
        self.order.is_empty()
    }

    /// Fold in one update. Ignored if the slot isn't declared; clears the
    /// slot if the value is empty. Returns whether anything actually changed
    /// (so the caller can skip a redraw for a no-op).
    pub fn apply(&mut self, update: WidgetUpdate) -> bool {
        if !self.order.iter().any(|s| s == &update.slot) {
            return false; // slot not part of this operator's band
        }
        if update.value.is_empty() {
            return self.values.remove(&update.slot).is_some();
        }
        let entry = (update.value, update.severity);
        match self.values.get(&update.slot) {
            Some(existing) if *existing == entry => false,
            _ => {
                self.values.insert(update.slot, entry);
                true
            }
        }
    }

    /// The currently-visible slots, in declared order — what the renderer
    /// draws. Skips declared-but-not-yet-updated slots.
    pub fn visible(&self) -> impl Iterator<Item = Slot> + '_ {
        self.order.iter().filter_map(move |name| {
            self.values.get(name).map(|(value, severity)| Slot {
                name: name.clone(),
                value: value.clone(),
                severity: *severity,
            })
        })
    }

    /// Whether anything is currently drawn — an enabled band with no live
    /// slots yet still reserves its strip (so content doesn't jump when the
    /// first indicator arrives).
    pub fn has_visible(&self) -> bool {
        self.order.iter().any(|name| self.values.contains_key(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(slot: &str, value: &str, severity: Option<Severity>) -> WidgetUpdate {
        WidgetUpdate { slot: slot.into(), value: value.into(), severity }
    }

    #[test]
    fn disabled_when_no_slots_declared() {
        let hud = Hud::new(vec![]);
        assert!(hud.is_disabled());
        assert_eq!(hud.visible().count(), 0);
    }

    #[test]
    fn ignores_updates_to_undeclared_slots() {
        let mut hud = Hud::new(vec!["battery".into()]);
        assert!(!hud.apply(update("temp", "51C", None)), "temp isn't declared");
        assert_eq!(hud.visible().count(), 0);
    }

    #[test]
    fn shows_declared_slots_in_declared_order_regardless_of_update_order() {
        let mut hud = Hud::new(vec!["battery".into(), "temp".into(), "alert".into()]);
        // Arrive out of order.
        assert!(hud.apply(update("temp", "51C", None)));
        assert!(hud.apply(update("battery", "42%", Some(Severity::Low))));
        let names: Vec<String> = hud.visible().map(|s| s.name).collect();
        assert_eq!(names, vec!["battery", "temp"], "declared order, not arrival order");
    }

    #[test]
    fn empty_value_clears_a_slot() {
        let mut hud = Hud::new(vec!["alert".into()]);
        assert!(hud.apply(update("alert", "INTRUSION", Some(Severity::Critical))));
        assert!(hud.has_visible());
        assert!(hud.apply(update("alert", "", None)), "clearing is a change");
        assert!(!hud.has_visible());
        assert!(!hud.apply(update("alert", "", None)), "clearing an already-empty slot is a no-op");
    }

    #[test]
    fn identical_update_is_a_no_op() {
        let mut hud = Hud::new(vec!["battery".into()]);
        assert!(hud.apply(update("battery", "42%", Some(Severity::Low))));
        assert!(!hud.apply(update("battery", "42%", Some(Severity::Low))), "same value, no change");
        assert!(hud.apply(update("battery", "41%", Some(Severity::Low))), "new value, change");
    }

    #[test]
    fn visible_carries_value_and_severity() {
        let mut hud = Hud::new(vec!["battery".into()]);
        hud.apply(update("battery", "42%", Some(Severity::Low)));
        let slot = hud.visible().next().unwrap();
        assert_eq!(slot.value, "42%");
        assert_eq!(slot.severity, Some(Severity::Low));
    }
}
