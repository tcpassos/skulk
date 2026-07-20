//! Physical navigation input, behind one small trait so the nav-mapping
//! layer doesn't care whether discrete GPIO buttons or (later) a rotary
//! encoder is actually wired up.

use std::collections::HashMap;

/// A raw event from some physical input source, named by the peripheral's
/// logical name — matching `contract::Peripheral::name` / `skulk.toml`'s
/// `[[peripherals]]` wiring, e.g. `"btn_a"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    Pressed(String),
    Released(String),
    /// A rotary encoder turned; signed steps (positive = one direction).
    /// No `InputSource` produces this yet — reserved for that later addition.
    Rotated(String, i8),
}

/// Anything that can report physical input events. [`crate::GpioButtons`]
/// (discrete pins) is the first implementation; a rotary encoder is a
/// natural second one behind this same trait, addable later without
/// touching callers.
#[async_trait::async_trait]
pub trait InputSource: Send {
    /// Waits for the next event. Returns `None` once the source is
    /// permanently exhausted (e.g. its underlying channel closed).
    async fn next_event(&mut self) -> Option<InputEvent>;
}

/// A device's logical navigation actions — deliberately small: a physical
/// implant has a handful of buttons or an encoder, not a keyboard. `Left`/
/// `Right` are optional — a 4-button device (up/down/select/back only) works
/// unchanged with neither bound; a 5-way joystick can additionally bind them
/// to move a [`crate::form::Form`]'s cursor between fields/edit points
/// without the submit/cancel side effect `Select`/`Back` have at the ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavAction {
    Up,
    Down,
    Left,
    Right,
    Select,
    Back,
}

impl NavAction {
    fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "up" => Some(Self::Up),
            "down" => Some(Self::Down),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "select" => Some(Self::Select),
            "back" => Some(Self::Back),
            _ => None,
        }
    }
}

/// Resolves a wired peripheral's name (e.g. `"btn_a"`) to a [`NavAction`].
///
/// Precedence: the operator's `skulk.toml` `[nav]` override, then the active
/// theme's `[nav]` section, then unbound. An unbound peripheral does
/// nothing and logs once — no positional guessing, matching this project's
/// "coarse, fail-clean" stance (see `skulkd::caps::detect`'s doc comment).
#[derive(Debug, Default)]
pub struct NavMap {
    operator_override: HashMap<String, NavAction>,
    theme: HashMap<String, NavAction>,
}

impl NavMap {
    pub fn new(operator_override: &HashMap<String, String>, theme: &HashMap<String, String>) -> Self {
        Self { operator_override: parse_map(operator_override), theme: parse_map(theme) }
    }

    pub fn resolve(&self, peripheral: &str) -> Option<NavAction> {
        self.operator_override.get(peripheral).or_else(|| self.theme.get(peripheral)).copied()
    }
}

fn parse_map(raw: &HashMap<String, String>) -> HashMap<String, NavAction> {
    raw.iter()
        .filter_map(|(peripheral, action)| match NavAction::parse(action) {
            Some(a) => Some((peripheral.clone(), a)),
            None => {
                tracing::warn!(peripheral, action, "nav: unknown action name, ignoring binding");
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn operator_override_wins_over_theme() {
        let nav = NavMap::new(&map(&[("btn_a", "down")]), &map(&[("btn_a", "up")]));
        assert_eq!(nav.resolve("btn_a"), Some(NavAction::Down));
    }

    #[test]
    fn falls_back_to_theme_when_operator_has_no_binding() {
        let nav = NavMap::new(&map(&[]), &map(&[("btn_b", "select")]));
        assert_eq!(nav.resolve("btn_b"), Some(NavAction::Select));
    }

    #[test]
    fn unbound_peripheral_resolves_to_none() {
        let nav = NavMap::new(&map(&[]), &map(&[]));
        assert_eq!(nav.resolve("btn_z"), None);
    }

    #[test]
    fn unknown_action_name_is_ignored_not_fatal() {
        let nav = NavMap::new(&map(&[("btn_a", "sideways")]), &map(&[]));
        assert_eq!(nav.resolve("btn_a"), None);
    }

    #[test]
    fn action_names_are_case_insensitive() {
        let nav = NavMap::new(&map(&[("btn_a", "UP")]), &map(&[]));
        assert_eq!(nav.resolve("btn_a"), Some(NavAction::Up));
    }

    /// A hardware-free `InputSource`, so the end-to-end wiring — read a raw
    /// event, resolve it through the nav map — is provable without a real
    /// `GpioButtons`.
    struct MockSource(std::collections::VecDeque<InputEvent>);

    #[async_trait::async_trait]
    impl InputSource for MockSource {
        async fn next_event(&mut self) -> Option<InputEvent> {
            self.0.pop_front()
        }
    }

    #[tokio::test]
    async fn input_source_events_resolve_through_the_nav_map() {
        let mut source = MockSource(
            [InputEvent::Pressed("btn_a".into()), InputEvent::Pressed("btn_unbound".into())]
                .into_iter()
                .collect(),
        );
        let nav = NavMap::new(&map(&[]), &map(&[("btn_a", "select")]));

        let InputEvent::Pressed(name) = source.next_event().await.unwrap() else {
            panic!("expected a Pressed event");
        };
        assert_eq!(nav.resolve(&name), Some(NavAction::Select));

        let InputEvent::Pressed(name) = source.next_event().await.unwrap() else {
            panic!("expected a Pressed event");
        };
        assert_eq!(nav.resolve(&name), None, "unbound peripherals resolve to no action");

        assert!(source.next_event().await.is_none());
    }
}
