//! Button-driven typed input for the on-device menu: a wizard of spinner
//! widgets built from an action's [`ParamSpec`]s, so param-taking modules run
//! from the screen without a keyboard. Pure state here — no
//! `embedded-graphics`, same split as [`crate::menu`]; the module ships no UI
//! code, only its declared param types drive which widget appears.
//!
//! Navigation folds onto the same four nav actions the menu uses. A widget may
//! have several *edit points* (a host has four octets); Select walks forward
//! through every edit point of the current field, then to the next field, and
//! finally submits; Back walks the same path in reverse and, at the very
//! start, cancels. Up/Down edit whatever edit point is current.

use contract::{Command, Invoke, ModuleId, ParamSpec, RawParams};
use serde_json::{Map, Value};

/// One editable value. Which variant a param becomes is decided purely from
/// its [`ParamSpec`] (see [`Widget::for_param`]).
#[derive(Debug, Clone, PartialEq)]
pub enum Widget {
    /// Four 0-255 octets — an IPv4 host. `cursor` selects the active octet.
    Octets { octets: [u8; 4], cursor: usize },
    /// A single bounded integer.
    Number { value: i64, min: i64, max: i64 },
    /// Two bounded integers rendered as `"start-end"` — a port range.
    /// `on_end` selects which end is being edited.
    Range { start: i64, end: i64, min: i64, max: i64, on_end: bool },
    /// A boolean flag.
    Toggle(bool),
    /// One of a closed set of values.
    Choice { options: Vec<String>, index: usize },
}

impl Widget {
    /// Build the widget for a param, or `None` if the param can't be entered
    /// with a spinner (free-text `string`/`mac`/unknown with no `allowed`
    /// set) — such a param keeps the action browse-only on the device.
    pub fn for_param(spec: &ParamSpec) -> Option<Widget> {
        // A closed value set always wins: it's a picker regardless of type.
        if !spec.allowed.is_empty() {
            let seed = spec.default.as_deref();
            let index = seed
                .and_then(|d| spec.allowed.iter().position(|v| v == d))
                .unwrap_or(0);
            return Some(Widget::Choice { options: spec.allowed.clone(), index });
        }
        let hint = spec.type_hint.as_deref().unwrap_or("");
        let seed = spec.default.as_deref().or(spec.example.as_deref());
        match hint {
            "host" | "ip" | "ipv4" => Some(Widget::Octets {
                octets: seed.and_then(parse_octets).unwrap_or([0, 0, 0, 0]),
                cursor: 0,
            }),
            "port-spec" | "range" => {
                let (min, max) = (spec.min.unwrap_or(1), spec.max.unwrap_or(65535));
                let (start, end) = seed.and_then(parse_range).unwrap_or((min, min));
                Some(Widget::Range { start: clamp(start, min, max), end: clamp(end, min, max), min, max, on_end: false })
            }
            "port" => number(spec, 1, 65535),
            "int" | "integer" | "uint" | "number" => number(spec, 0, 65535),
            "bool" | "boolean" | "flag" => {
                Some(Widget::Toggle(matches!(seed, Some("true") | Some("1") | Some("yes"))))
            }
            _ => None,
        }
    }

    /// Step the current edit point up.
    pub fn up(&mut self) {
        match self {
            Widget::Octets { octets, cursor } => octets[*cursor] = octets[*cursor].saturating_add(1),
            Widget::Number { value, min, max } => *value = clamp(*value + 1, *min, *max),
            Widget::Range { start, end, min, max, on_end } => {
                let v = if *on_end { end } else { start };
                *v = clamp(*v + 1, *min, *max);
            }
            Widget::Toggle(b) => *b = !*b,
            Widget::Choice { options, index } => {
                if !options.is_empty() {
                    *index = (*index + 1) % options.len();
                }
            }
        }
    }

    /// Step the current edit point down.
    pub fn down(&mut self) {
        match self {
            Widget::Octets { octets, cursor } => octets[*cursor] = octets[*cursor].saturating_sub(1),
            Widget::Number { value, min, max } => *value = clamp(*value - 1, *min, *max),
            Widget::Range { start, end, min, max, on_end } => {
                let v = if *on_end { end } else { start };
                *v = clamp(*v - 1, *min, *max);
            }
            Widget::Toggle(b) => *b = !*b,
            Widget::Choice { options, index } => {
                if !options.is_empty() {
                    *index = (*index + options.len() - 1) % options.len();
                }
            }
        }
    }

    /// Advance the internal edit point (e.g. next octet). Returns `false` when
    /// already at the last one, so the caller advances to the next field.
    fn advance(&mut self) -> bool {
        match self {
            Widget::Octets { cursor, .. } if *cursor < 3 => {
                *cursor += 1;
                true
            }
            Widget::Range { on_end, .. } if !*on_end => {
                *on_end = true;
                true
            }
            _ => false,
        }
    }

    /// Retreat the internal edit point. Returns `false` when already at the
    /// first one, so the caller moves to the previous field.
    fn retreat(&mut self) -> bool {
        match self {
            Widget::Octets { cursor, .. } if *cursor > 0 => {
                *cursor -= 1;
                true
            }
            Widget::Range { on_end, .. } if *on_end => {
                *on_end = false;
                true
            }
            _ => false,
        }
    }

    /// Reset the internal edit point to its first position (when a field is
    /// entered fresh from either direction the cursor starts predictably).
    fn reset_cursor(&mut self, to_end: bool) {
        match self {
            Widget::Octets { cursor, .. } => *cursor = if to_end { 3 } else { 0 },
            Widget::Range { on_end, .. } => *on_end = to_end,
            _ => {}
        }
    }

    /// The parameter value this widget currently represents.
    pub fn value(&self) -> Value {
        match self {
            Widget::Octets { octets, .. } => {
                Value::String(format!("{}.{}.{}.{}", octets[0], octets[1], octets[2], octets[3]))
            }
            Widget::Number { value, .. } => Value::Number((*value).into()),
            Widget::Range { start, end, .. } => Value::String(format!("{start}-{end}")),
            Widget::Toggle(b) => Value::Bool(*b),
            Widget::Choice { options, index } => {
                // Infer the option's JSON type (so "80" -> number, "true" ->
                // bool), matching the CLI/TUI's key=value inference.
                options
                    .get(*index)
                    .map(|s| serde_json::from_str::<Value>(s).unwrap_or_else(|_| Value::String(s.clone())))
                    .unwrap_or(Value::Null)
            }
        }
    }

    /// A short human-readable rendering of the current value, for the display.
    pub fn display(&self) -> String {
        match self {
            Widget::Octets { octets, cursor } => {
                let parts: Vec<String> = octets
                    .iter()
                    .enumerate()
                    .map(|(i, o)| if i == *cursor { format!("[{o}]") } else { o.to_string() })
                    .collect();
                parts.join(".")
            }
            Widget::Number { value, .. } => value.to_string(),
            Widget::Range { start, end, on_end, .. } => {
                if *on_end {
                    format!("{start}-[{end}]")
                } else {
                    format!("[{start}]-{end}")
                }
            }
            Widget::Toggle(b) => if *b { "yes" } else { "no" }.to_string(),
            Widget::Choice { options, index } => options.get(*index).cloned().unwrap_or_default(),
        }
    }
}

/// One field of a [`Form`]: a param name plus its editing widget.
#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub widget: Widget,
}

/// A wizard collecting one action's parameters via spinner widgets.
#[derive(Debug, Clone)]
pub struct Form {
    module: ModuleId,
    action: String,
    fields: Vec<Field>,
    cursor: usize,
}

/// What a `Back` press did to a form.
#[derive(Debug, PartialEq, Eq)]
pub enum Back {
    /// Stayed in the form (moved a cursor back).
    Stay,
    /// Backed out of the first edit point — the form should close.
    Cancel,
}

impl Form {
    /// Build a form for `action`'s params, or `None` if the action can't be
    /// run from the device — i.e. a *required* param has no spinner widget
    /// (free text). Optional params that lack a widget are simply skipped;
    /// optional params that have one are included (pre-seeded from default).
    pub fn for_action(module: ModuleId, action: impl Into<String>, params: &[ParamSpec]) -> Option<Form> {
        let mut fields = Vec::new();
        for spec in params {
            match Widget::for_param(spec) {
                Some(widget) => fields.push(Field { name: spec.name.clone(), widget }),
                None if spec.required => return None, // required free-text: not device-runnable
                None => {} // optional free-text: leave to the module's default
            }
        }
        if fields.is_empty() {
            return None; // nothing to edit — caller should invoke directly instead
        }
        Some(Form { module, action: action.into(), fields, cursor: 0 })
    }

    pub fn module(&self) -> &ModuleId {
        &self.module
    }

    pub fn action(&self) -> &str {
        &self.action
    }

    pub fn fields(&self) -> &[Field] {
        &self.fields
    }

    /// Index of the field currently being edited.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Step the current edit point up/down.
    pub fn up(&mut self) {
        self.fields[self.cursor].widget.up();
    }

    pub fn down(&mut self) {
        self.fields[self.cursor].widget.down();
    }

    /// Select: advance within the current widget, then to the next field, and
    /// on the final edit point build and return the [`Command::Invoke`].
    pub fn select(&mut self) -> Option<Command> {
        if self.fields[self.cursor].widget.advance() {
            return None; // moved to the next octet/range-end of the same field
        }
        if self.cursor + 1 < self.fields.len() {
            self.cursor += 1;
            self.fields[self.cursor].widget.reset_cursor(false);
            return None;
        }
        Some(self.to_invoke())
    }

    /// Back: retreat within the current widget, then to the previous field;
    /// at the very first edit point, signal the form should be cancelled.
    pub fn back(&mut self) -> Back {
        if self.fields[self.cursor].widget.retreat() {
            return Back::Stay;
        }
        if self.cursor > 0 {
            self.cursor -= 1;
            self.fields[self.cursor].widget.reset_cursor(true);
            return Back::Stay;
        }
        Back::Cancel
    }

    fn to_invoke(&self) -> Command {
        let mut map = Map::new();
        for field in &self.fields {
            map.insert(field.name.clone(), field.widget.value());
        }
        Command::Invoke(Invoke {
            module: self.module.clone(),
            action: self.action.clone(),
            params: RawParams(Value::Object(map)),
            timeout_ms: None,
        })
    }
}

fn number(spec: &ParamSpec, dmin: i64, dmax: i64) -> Option<Widget> {
    let (min, max) = (spec.min.unwrap_or(dmin), spec.max.unwrap_or(dmax));
    let seed = spec.default.as_deref().or(spec.example.as_deref());
    let value = seed.and_then(|s| s.parse::<i64>().ok()).unwrap_or(min);
    Some(Widget::Number { value: clamp(value, min, max), min, max })
}

fn clamp(v: i64, min: i64, max: i64) -> i64 {
    v.max(min).min(max)
}

fn parse_octets(s: &str) -> Option<[u8; 4]> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let mut out = [0u8; 4];
    for (i, p) in parts.iter().enumerate() {
        out[i] = p.parse().ok()?;
    }
    Some(out)
}

fn parse_range(s: &str) -> Option<(i64, i64)> {
    let (a, b) = s.split_once('-')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host_param() -> ParamSpec {
        ParamSpec::required("target", "host", "IP").with_example("10.0.0.1")
    }
    fn port_param() -> ParamSpec {
        ParamSpec::optional("port", "port", "port").with_default("80")
    }

    #[test]
    fn host_widget_seeds_from_example_and_edits_octets() {
        let mut w = Widget::for_param(&host_param()).unwrap();
        assert_eq!(w.value(), Value::String("10.0.0.1".into()));
        w.up(); // first octet 10 -> 11
        assert_eq!(w.value(), Value::String("11.0.0.1".into()));
    }

    #[test]
    fn number_widget_clamps_to_bounds() {
        let spec = ParamSpec::required("port", "port", "p"); // default port range 1..65535
        let mut w = Widget::for_param(&spec).unwrap();
        // seeds at min (1); down must not go below min.
        assert_eq!(w.value(), Value::Number(1.into()));
        w.down();
        assert_eq!(w.value(), Value::Number(1.into()), "clamped at min");
    }

    #[test]
    fn choice_widget_from_allowed_infers_value_type() {
        let spec = ParamSpec::optional("mode", "enum", "m").with_allowed(["connect", "syn"]);
        let mut w = Widget::for_param(&spec).unwrap();
        assert_eq!(w.value(), Value::String("connect".into()));
        w.up();
        assert_eq!(w.value(), Value::String("syn".into()));
        w.up(); // wraps
        assert_eq!(w.value(), Value::String("connect".into()));
    }

    #[test]
    fn toggle_widget_flips() {
        let spec = ParamSpec::optional("verbose", "bool", "v");
        let mut w = Widget::for_param(&spec).unwrap();
        assert_eq!(w.value(), Value::Bool(false));
        w.up();
        assert_eq!(w.value(), Value::Bool(true));
    }

    #[test]
    fn range_widget_edits_both_ends() {
        let spec = ParamSpec::optional("ports", "port-spec", "r").with_example("20-25");
        let mut w = Widget::for_param(&spec).unwrap();
        assert_eq!(w.value(), Value::String("20-25".into()));
        assert!(w.advance(), "advance moves onto the end value");
        w.up(); // end 25 -> 26
        assert_eq!(w.value(), Value::String("20-26".into()));
        assert!(!w.advance(), "no third edit point");
    }

    #[test]
    fn free_text_param_has_no_widget() {
        let spec = ParamSpec::required("name", "string", "a name");
        assert!(Widget::for_param(&spec).is_none());
    }

    #[test]
    fn form_refuses_when_a_required_param_is_free_text() {
        let params = vec![
            host_param(),
            ParamSpec::required("label", "string", "free text"),
        ];
        assert!(Form::for_action(ModuleId::from("x.y"), "go", &params).is_none());
    }

    #[test]
    fn form_skips_optional_free_text_but_keeps_spinnable_fields() {
        let params = vec![
            host_param(),
            ParamSpec::optional("note", "string", "free text"), // skipped
            port_param(),
        ];
        let form = Form::for_action(ModuleId::from("net.ports"), "scan", &params).unwrap();
        assert_eq!(form.fields().len(), 2, "host + port; the free-text note is skipped");
    }

    #[test]
    fn select_walks_octets_then_fields_then_submits() {
        let params = vec![host_param(), port_param()];
        let mut form = Form::for_action(ModuleId::from("net.ports"), "scan", &params).unwrap();
        // host has 4 octets: 3 advances stay within the field...
        assert!(form.select().is_none());
        assert!(form.select().is_none());
        assert!(form.select().is_none());
        // 4th select leaves host for the port field...
        assert!(form.select().is_none());
        assert_eq!(form.cursor(), 1, "now on the port field");
        // port is single-point: next select submits.
        let cmd = form.select().expect("final select submits");
        match cmd {
            Command::Invoke(inv) => {
                assert_eq!(inv.module, ModuleId::from("net.ports"));
                assert_eq!(inv.action, "scan");
                assert_eq!(inv.params.0["target"], Value::String("10.0.0.1".into()));
                assert_eq!(inv.params.0["port"], Value::Number(80.into()));
            }
            _ => panic!("expected an invoke"),
        }
    }

    #[test]
    fn back_from_the_first_edit_point_cancels() {
        let params = vec![port_param()];
        let mut form = Form::for_action(ModuleId::from("net.ports"), "scan", &params).unwrap();
        assert_eq!(form.back(), Back::Cancel);
    }

    #[test]
    fn back_retreats_through_octets_before_leaving_the_field() {
        let params = vec![host_param()];
        let mut form = Form::for_action(ModuleId::from("net.ports"), "scan", &params).unwrap();
        form.select(); // octet cursor 0 -> 1
        form.select(); // 1 -> 2
        assert_eq!(form.back(), Back::Stay); // 2 -> 1
        assert_eq!(form.back(), Back::Stay); // 1 -> 0
        assert_eq!(form.back(), Back::Cancel); // at first octet of only field
    }
}
