//! On-device menu: browse the `Manifest` and invoke param-less actions via
//! physical buttons. Pure state here — no `embedded-graphics`, same split as
//! [`crate::NavMap`]; [`crate::Renderer::draw_menu`] does the pixel work,
//! and [`App`] is the small state machine deciding which screen is active.

use contract::{Command, Envelope, Invoke, Manifest, ModuleId, ParamSpec, RawParams, ViewLine, ViewManifest};

use crate::form::{Back, Form};
use crate::input::NavAction;

/// One row of the flattened, grouped module list — a scaled-down version of
/// `skulk-tui`'s `Row` (no description text, no capability prose): a tiny
/// screen has room for a name and a single marker, not a sentence.
#[derive(Debug, Clone)]
pub enum Row {
    Group(String),
    Module {
        id: ModuleId,
        action: String,
        /// Whether Select can invoke this directly with no further input —
        /// i.e. the action declares no required params.
        invokable: bool,
        /// The action's declared params, so a params-taking row can build a
        /// [`Form`] of spinner widgets on Select (see [`App::apply_nav`]).
        params: Vec<ParamSpec>,
    },
}

/// Browsable, invokable menu built from a `Manifest`. Rebuild it (via
/// [`Menu::from_manifest`]) whenever a fresh `Manifest` arrives — e.g. after
/// a `Describe` refresh — the same way `skulk-tui` rebuilds its module tree.
#[derive(Debug)]
pub struct Menu {
    rows: Vec<Row>,
    selected: usize,
}

impl Menu {
    pub fn from_manifest(manifest: &Manifest) -> Self {
        let mut modules = manifest.modules.clone();
        modules.sort_by(|a, b| a.id.0.cmp(&b.id.0));

        let mut rows = Vec::new();
        let mut group = String::new();
        for md in &modules {
            let g = md.id.0.split_once('.').map(|(g, _)| g).unwrap_or("").to_string();
            if g != group {
                rows.push(Row::Group(g.clone()));
                group = g;
            }
            for action in &md.actions {
                rows.push(Row::Module {
                    id: md.id.clone(),
                    action: action.name.clone(),
                    invokable: !action.params.iter().any(|p| p.required),
                    params: action.params.clone(),
                });
            }
        }
        let selected = rows.iter().position(|r| matches!(r, Row::Module { .. })).unwrap_or(0);
        Self { rows, selected }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn move_up(&mut self) {
        let mut i = self.selected;
        while i > 0 {
            i -= 1;
            if matches!(self.rows[i], Row::Module { .. }) {
                self.selected = i;
                break;
            }
        }
    }

    pub fn move_down(&mut self) {
        let mut i = self.selected;
        while i + 1 < self.rows.len() {
            i += 1;
            if matches!(self.rows[i], Row::Module { .. }) {
                self.selected = i;
                break;
            }
        }
    }

    /// Invoke the selected row if it's runnable without params. Returns
    /// `None` (does nothing) for group rows or actions that need params.
    pub fn activate(&self) -> Option<Command> {
        match self.rows.get(self.selected) {
            Some(Row::Module { id, action, invokable: true, .. }) => Some(Command::Invoke(Invoke {
                module: id.clone(),
                action: action.clone(),
                params: RawParams::default(),
                timeout_ms: None,
            })),
            _ => None,
        }
    }

    /// The currently-selected module row (not a group header), if any — lets
    /// the app read its params to build a [`Form`].
    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected).filter(|r| matches!(r, Row::Module { .. }))
    }
}

/// Which of the LCD's screens is currently shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// The live tactical `ViewManifest` — the default, matches today's
    /// behaviour with no buttons wired at all.
    Status,
    /// The browsable/invokable module menu.
    Menu,
    /// The spinner form for a params-taking action selected from the menu.
    Form,
}

/// What one resolved nav action did to an open [`Form`] — kept separate from
/// mutating [`App`] so the form's borrow ends before the screen is changed.
enum FormOutcome {
    Stay,
    Submit(Command),
    CloseToMenu,
}

/// Ties the menu, an optional input form, the latest tactical view, and
/// physical input together: which screen is showing, and what one resolved
/// [`NavAction`] does to it.
pub struct App {
    screen: Screen,
    menu: Menu,
    form: Option<Form>,
    last_view: Option<ViewManifest>,
}

impl App {
    pub fn new(manifest: &Manifest) -> Self {
        Self { screen: Screen::Status, menu: Menu::from_manifest(manifest), form: None, last_view: None }
    }

    pub fn screen(&self) -> Screen {
        self.screen
    }

    pub fn menu(&self) -> &Menu {
        &self.menu
    }

    /// The open input form, if the current screen is [`Screen::Form`].
    pub fn form(&self) -> Option<&Form> {
        self.form.as_ref()
    }

    /// Fold in a live tactical update. Does not change which screen is
    /// active -- a scan running in the background shouldn't yank the
    /// operator out of the menu they're browsing.
    pub fn apply_view(&mut self, view: ViewManifest) {
        self.last_view = Some(view);
    }

    /// The status screen's content: the latest tactical view, or a small
    /// placeholder before anything has ever arrived.
    pub fn status_view(&self) -> ViewManifest {
        self.last_view.clone().unwrap_or_else(|| ViewManifest {
            screen: "skulk".into(),
            lines: vec![ViewLine { label: "status".into(), value: "idle".into(), severity: None }],
        })
    }

    /// Route one resolved nav action. Returns a `Command` when Select just
    /// activated a runnable menu row -- the caller sends it to the engine.
    pub fn apply_nav(&mut self, action: NavAction) -> Option<Command> {
        match self.screen {
            // Any press wakes the menu up; the press itself isn't also
            // consumed as a menu action (surprising to move the cursor on
            // the very button press that opened the menu).
            Screen::Status => {
                self.screen = Screen::Menu;
                None
            }
            Screen::Menu => match action {
                NavAction::Up => {
                    self.menu.move_up();
                    None
                }
                NavAction::Down => {
                    self.menu.move_down();
                    None
                }
                NavAction::Back => {
                    self.screen = Screen::Status;
                    None
                }
                NavAction::Select => {
                    // Param-less action: invoke it and switch to Status to
                    // watch it run.
                    if let Some(cmd) = self.menu.activate() {
                        self.screen = Screen::Status;
                        return Some(cmd);
                    }
                    // Params-taking action: open a spinner form, but only if
                    // every required param is spinner-fillable. A required
                    // free-text param leaves the action browse-only (nothing
                    // happens here — run it from the CLI/TUI instead).
                    let form = self.menu.selected_row().and_then(|row| match row {
                        Row::Module { id, action, params, .. } => {
                            Form::for_action(id.clone(), action.clone(), params)
                        }
                        _ => None,
                    });
                    if let Some(form) = form {
                        self.form = Some(form);
                        self.screen = Screen::Form;
                    }
                    None
                }
            },
            Screen::Form => {
                let outcome = match self.form.as_mut() {
                    None => FormOutcome::CloseToMenu, // defensive: no form -> leave the screen
                    Some(form) => match action {
                        NavAction::Up => {
                            form.up();
                            FormOutcome::Stay
                        }
                        NavAction::Down => {
                            form.down();
                            FormOutcome::Stay
                        }
                        NavAction::Select => match form.select() {
                            Some(cmd) => FormOutcome::Submit(cmd),
                            None => FormOutcome::Stay,
                        },
                        NavAction::Back => match form.back() {
                            Back::Stay => FormOutcome::Stay,
                            Back::Cancel => FormOutcome::CloseToMenu,
                        },
                    },
                };
                match outcome {
                    FormOutcome::Stay => None,
                    FormOutcome::Submit(cmd) => {
                        self.form = None;
                        self.screen = Screen::Status; // watch it run
                        Some(cmd)
                    }
                    FormOutcome::CloseToMenu => {
                        self.form = None;
                        self.screen = Screen::Menu;
                        None
                    }
                }
            }
        }
    }
}

/// Wrap a `Command` the way the engine expects to receive one, stamped with
/// the current wall-clock time (matching real invocations, not the `0`
/// placeholder loopback tests use).
pub(crate) fn envelope(command: Command) -> Envelope {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    Envelope::new(contract::Body::Command(command), now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use contract::{ActionSpec, ModuleDescriptor, ParamSpec, Tactic};

    fn sample_manifest() -> Manifest {
        Manifest {
            protocol: 1,
            implant: contract::ImplantInfo { id: "t".into(), hardware: "t".into(), firmware: "0".into() },
            modules: vec![
                ModuleDescriptor {
                    id: ModuleId::from("net.ports"),
                    version: "0".into(),
                    tactic: Some(Tactic::Discovery),
                    actions: vec![ActionSpec {
                        name: "scan".into(),
                        description: None,
                        params: vec![ParamSpec::required("target", "host", "target")],
                        params_schema: None,
                    }],
                    requires: vec![],
                },
                ModuleDescriptor {
                    id: ModuleId::from("sys.info"),
                    version: "0".into(),
                    tactic: None,
                    actions: vec![ActionSpec {
                        name: "get".into(),
                        description: None,
                        params: vec![ParamSpec::optional("verbose", "bool", "verbose")],
                        params_schema: None,
                    }],
                    requires: vec![],
                },
            ],
            capabilities: vec![],
            peripherals: vec![],
        }
    }

    #[test]
    fn builds_grouped_rows_and_flags_invokability() {
        let menu = Menu::from_manifest(&sample_manifest());
        assert!(matches!(&menu.rows()[0], Row::Group(g) if g == "net"));
        match &menu.rows()[1] {
            Row::Module { id, action, invokable, .. } => {
                assert_eq!(*id, ModuleId::from("net.ports"));
                assert_eq!(action, "scan");
                assert!(!invokable, "scan requires a target param");
            }
            _ => panic!("expected net.ports scan row"),
        }
        assert!(matches!(&menu.rows()[2], Row::Group(g) if g == "sys"));
        match &menu.rows()[3] {
            Row::Module { action, invokable, .. } => {
                assert_eq!(action, "get");
                assert!(invokable, "get has no required params");
            }
            _ => panic!("expected sys.info get row"),
        }
    }

    #[test]
    fn navigation_skips_groups_and_wraps_at_ends() {
        let mut menu = Menu::from_manifest(&sample_manifest());
        assert_eq!(menu.selected(), 1); // first module row
        menu.move_up(); // already at the top -- stays
        assert_eq!(menu.selected(), 1);
        menu.move_down(); // skips Group("sys") at index 2
        assert_eq!(menu.selected(), 3);
        menu.move_down(); // already at the bottom -- stays
        assert_eq!(menu.selected(), 3);
        menu.move_up();
        assert_eq!(menu.selected(), 1);
    }

    #[test]
    fn activate_on_a_group_row_does_nothing() {
        let mut menu = Menu::from_manifest(&sample_manifest());
        menu.move_up(); // no-op, but exercise the boundary
        // Force selection onto the Group row directly to prove groups never invoke.
        menu.selected = 0;
        assert!(menu.activate().is_none());
    }

    #[test]
    fn activate_on_a_params_required_row_does_nothing() {
        let menu = Menu::from_manifest(&sample_manifest());
        assert_eq!(menu.selected(), 1); // net.ports scan -- needs `target`
        assert!(menu.activate().is_none());
    }

    #[test]
    fn activate_on_an_invokable_row_returns_the_invoke() {
        let mut menu = Menu::from_manifest(&sample_manifest());
        menu.move_down(); // -> sys.info get, invokable
        match menu.activate() {
            Some(Command::Invoke(inv)) => {
                assert_eq!(inv.module, ModuleId::from("sys.info"));
                assert_eq!(inv.action, "get");
            }
            other => panic!("expected an invoke, got {other:?}"),
        }
    }

    #[test]
    fn first_press_on_status_opens_the_menu_without_moving_the_cursor() {
        let mut app = App::new(&sample_manifest());
        assert_eq!(app.screen(), Screen::Status);
        let cmd = app.apply_nav(NavAction::Down);
        assert!(cmd.is_none());
        assert_eq!(app.screen(), Screen::Menu);
        assert_eq!(app.menu().selected(), 1, "the opening press shouldn't also move the cursor");
    }

    #[test]
    fn back_returns_to_status_without_invoking() {
        let mut app = App::new(&sample_manifest());
        app.apply_nav(NavAction::Down); // open menu
        let cmd = app.apply_nav(NavAction::Back);
        assert!(cmd.is_none());
        assert_eq!(app.screen(), Screen::Status);
    }

    #[test]
    fn selecting_a_runnable_row_switches_back_to_status() {
        let mut app = App::new(&sample_manifest());
        app.apply_nav(NavAction::Down); // open menu, selected on net.ports scan
        app.apply_nav(NavAction::Down); // move to sys.info get (invokable)
        let cmd = app.apply_nav(NavAction::Select);
        assert!(cmd.is_some());
        assert_eq!(app.screen(), Screen::Status, "running an action returns to the live view");
    }

    #[test]
    fn selecting_a_spinnable_params_action_opens_a_form() {
        let mut app = App::new(&sample_manifest());
        app.apply_nav(NavAction::Down); // open menu, selected on net.ports scan (needs a host target)
        let cmd = app.apply_nav(NavAction::Select);
        assert!(cmd.is_none(), "opening a form doesn't invoke yet");
        assert_eq!(app.screen(), Screen::Form);
        assert_eq!(app.form().unwrap().action(), "scan");
    }

    #[test]
    fn filling_a_host_form_and_submitting_invokes_with_the_value() {
        let mut app = App::new(&sample_manifest());
        app.apply_nav(NavAction::Down); // open menu on net.ports scan
        app.apply_nav(NavAction::Select); // open the form (host field, 4 octets)
        // Select walks octet 0->1->2->3, then the final Select submits.
        assert!(app.apply_nav(NavAction::Select).is_none());
        assert!(app.apply_nav(NavAction::Select).is_none());
        assert!(app.apply_nav(NavAction::Select).is_none());
        let cmd = app.apply_nav(NavAction::Select).expect("final select submits");
        match cmd {
            Command::Invoke(inv) => {
                assert_eq!(inv.module, ModuleId::from("net.ports"));
                assert_eq!(inv.action, "scan");
                // No example in the sample spec -> host seeds at 0.0.0.0.
                assert_eq!(inv.params.0["target"], serde_json::json!("0.0.0.0"));
            }
            _ => panic!("expected an invoke"),
        }
        assert_eq!(app.screen(), Screen::Status, "after submit, watch it run");
        assert!(app.form().is_none());
    }

    #[test]
    fn cancelling_a_form_returns_to_the_menu() {
        let mut app = App::new(&sample_manifest());
        app.apply_nav(NavAction::Down); // menu on net.ports scan
        app.apply_nav(NavAction::Select); // open form
        assert_eq!(app.screen(), Screen::Form);
        // Back retreats through the 4 octets (starts at octet 0), so a single
        // Back from the fresh form's first octet cancels straight away.
        app.apply_nav(NavAction::Back);
        assert_eq!(app.screen(), Screen::Menu, "back from the first edit point returns to the menu");
        assert!(app.form().is_none());
    }

    #[test]
    fn selecting_a_free_text_required_action_stays_on_the_menu() {
        // A required free-text param can't be spun -> the action is
        // browse-only on the device; Select does nothing.
        let manifest = Manifest {
            protocol: 1,
            implant: contract::ImplantInfo { id: "t".into(), hardware: "t".into(), firmware: "0".into() },
            modules: vec![ModuleDescriptor {
                id: ModuleId::from("sys.note"),
                version: "0".into(),
                tactic: None,
                actions: vec![ActionSpec {
                    name: "write".into(),
                    description: None,
                    params: vec![ParamSpec::required("text", "string", "free text")],
                    params_schema: None,
                }],
                requires: vec![],
            }],
            capabilities: vec![],
            peripherals: vec![],
        };
        let mut app = App::new(&manifest);
        app.apply_nav(NavAction::Down); // open menu on sys.note write
        let cmd = app.apply_nav(NavAction::Select);
        assert!(cmd.is_none());
        assert_eq!(app.screen(), Screen::Menu, "browse-only action: Select is a no-op");
    }

    #[test]
    fn status_view_falls_back_to_a_placeholder_before_any_scan_ran() {
        let app = App::new(&sample_manifest());
        let view = app.status_view();
        assert_eq!(view.screen, "skulk");
    }

    #[test]
    fn status_view_reflects_the_latest_applied_view() {
        let mut app = App::new(&sample_manifest());
        app.apply_view(ViewManifest { screen: "net.ports".into(), lines: vec![] });
        assert_eq!(app.status_view().screen, "net.ports");
    }
}
