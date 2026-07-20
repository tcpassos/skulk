//! On-device menu: browse the `Manifest` and invoke param-less actions via
//! physical buttons. Pure state here — no `embedded-graphics`, same split as
//! [`crate::NavMap`]; [`crate::Renderer::draw_menu`] does the pixel work,
//! and [`App`] is the small state machine deciding which screen is active.

use contract::{
    Command, Envelope, Invoke, LootEntry, Manifest, ModuleId, ParamSpec, RawParams, ViewLine, ViewManifest,
};

use crate::form::{Back, Form};
use crate::input::NavAction;
use crate::textview::TextView;

/// One row of the flattened, grouped module list — a scaled-down version of
/// `skulk-tui`'s `Row` (no description text, no capability prose): a tiny
/// screen has room for a name and a single marker, not a sentence. Loot
/// entries live in their own trailing "loot" group, appended/refreshed live
/// as loot arrives (see [`Menu::set_loot`]/[`Menu::note_loot`]) rather than
/// being part of the static manifest-derived rows above them.
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
    /// One stored loot item; Select opens its content in a [`TextView`]
    /// (see [`NavOutcome::FetchLoot`]).
    Loot(LootEntry),
}

/// Browsable, invokable menu built from a `Manifest`. Rebuild it (via
/// [`Menu::from_manifest`]) whenever a fresh `Manifest` arrives — e.g. after
/// a `Describe` refresh — the same way `skulk-tui` rebuilds its module tree.
#[derive(Debug)]
pub struct Menu {
    rows: Vec<Row>,
    selected: usize,
    /// How many of `rows` are the static manifest-derived rows, i.e. where
    /// the loot section (if any) starts — see [`Menu::rebuild_loot_rows`].
    module_row_count: usize,
    loot: Vec<LootEntry>,
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
        let module_row_count = rows.len();
        let selected = rows.iter().position(|r| matches!(r, Row::Module { .. })).unwrap_or(0);
        Self { rows, selected, module_row_count, loot: Vec::new() }
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
            if !matches!(self.rows[i], Row::Group(_)) {
                self.selected = i;
                break;
            }
        }
    }

    pub fn move_down(&mut self) {
        let mut i = self.selected;
        while i + 1 < self.rows.len() {
            i += 1;
            if !matches!(self.rows[i], Row::Group(_)) {
                self.selected = i;
                break;
            }
        }
    }

    /// Invoke the selected row if it's runnable without params. Returns
    /// `None` (does nothing) for group/loot rows or actions that need params.
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

    /// The currently-selected module row (not a group header or a loot row),
    /// if any — lets the app read its params to build a [`Form`].
    pub fn selected_row(&self) -> Option<&Row> {
        self.rows.get(self.selected).filter(|r| matches!(r, Row::Module { .. }))
    }

    /// The currently-selected loot entry, if the cursor is on one.
    pub fn selected_loot(&self) -> Option<&LootEntry> {
        match self.rows.get(self.selected) {
            Some(Row::Loot(entry)) => Some(entry),
            _ => None,
        }
    }

    /// Replace the whole loot backlog — e.g. an initial backfill query run
    /// once at startup (see `run_app`) — and rebuild the trailing "loot"
    /// group from it.
    pub fn set_loot(&mut self, entries: Vec<LootEntry>) {
        self.loot = entries;
        self.rebuild_loot_rows();
    }

    /// Fold in one freshly-stored item (`Event::LootStored`): updates it in
    /// place if the key is already known (a module can overwrite its own
    /// key), otherwise inserts it at the front -- `self.loot` is kept
    /// newest-first (matching the engine's query order, see
    /// `Menu::set_loot`), so a brand-new item belongs at the top, not the
    /// bottom. Caps at [`crate::RECENT_LOOT_LIMIT`] so a long-running
    /// daemon's loot list can't grow forever; older entries are still reachable via
    /// `skulk loot --prefix ...`, just not in this live view.
    pub fn note_loot(&mut self, entry: LootEntry) {
        match self.loot.iter_mut().find(|e| e.key == entry.key) {
            Some(existing) => *existing = entry,
            None => {
                self.loot.insert(0, entry);
                self.loot.truncate(crate::RECENT_LOOT_LIMIT);
            }
        }
        self.rebuild_loot_rows();
    }

    /// Rebuild the trailing "loot" group from `self.loot`, keeping the
    /// static module rows before it untouched. Only re-homes the cursor if
    /// it became invalid (past the end, or now sitting on a group header
    /// because the section shrank) — otherwise leaves it exactly where it
    /// was, matching `Menu::from_manifest`'s "fail clean" stance.
    fn rebuild_loot_rows(&mut self) {
        self.rows.truncate(self.module_row_count);
        if !self.loot.is_empty() {
            self.rows.push(Row::Group("loot".to_string()));
            self.rows.extend(self.loot.iter().cloned().map(Row::Loot));
        }
        if self.selected >= self.rows.len() || matches!(self.rows.get(self.selected), Some(Row::Group(_))) {
            self.selected = self.rows.iter().position(|r| !matches!(r, Row::Group(_))).unwrap_or(0);
        }
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
    /// A fetched loot item's content (see [`crate::TextView`]).
    TextView,
}

/// What one resolved nav action did to an open [`Form`] — kept separate from
/// mutating [`App`] so the form's borrow ends before the screen is changed.
enum FormOutcome {
    Stay,
    Submit(Command),
    CloseToMenu,
}

/// What [`App::apply_nav`] resolved a [`NavAction`] into — a plain `Option`
/// isn't enough once more than one kind of "the caller needs to do
/// something async" exists (send an `Invoke`, or fetch a loot item's bytes).
/// `apply_nav` itself stays synchronous and I/O-free; `run_app` is the one
/// place that actually awaits the engine, matching this crate's existing
/// menu/form split ("pure state, no embedded-graphics").
#[derive(Debug, PartialEq)]
pub enum NavOutcome {
    /// Nothing further to do.
    None,
    /// Send this command to the engine.
    Invoke(Command),
    /// Fetch this loot key's bytes (`Engine::loot_get`) and hand the result
    /// to [`App::show_loot_content`]/[`App::show_loot_missing`].
    FetchLoot(String),
}

/// Ties the menu, an optional input form, the latest tactical view, and
/// physical input together: which screen is showing, and what one resolved
/// [`NavAction`] does to it.
pub struct App {
    screen: Screen,
    menu: Menu,
    form: Option<Form>,
    text_view: Option<TextView>,
    last_view: Option<ViewManifest>,
}

impl App {
    pub fn new(manifest: &Manifest) -> Self {
        Self {
            screen: Screen::Status,
            menu: Menu::from_manifest(manifest),
            form: None,
            text_view: None,
            last_view: None,
        }
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

    /// The open text viewer, if the current screen is [`Screen::TextView`].
    pub fn text_view(&self) -> Option<&TextView> {
        self.text_view.as_ref()
    }

    /// Seed the loot list from an initial backfill query (`run_app` issues
    /// one at startup via `Engine::loot_query`) — after this, live
    /// `Event::LootStored`s keep it current via [`App::note_loot`].
    pub fn set_loot_backlog(&mut self, entries: Vec<LootEntry>) {
        self.menu.set_loot(entries);
    }

    /// Fold in one freshly-stored loot item.
    pub fn note_loot(&mut self, entry: LootEntry) {
        self.menu.note_loot(entry);
    }

    /// Open the text viewer with a loot item's fetched content — called by
    /// `run_app` once its (async, in-process) `Engine::loot_get` resolves.
    /// Best-effort UTF-8 decode; anything else shows a placeholder instead
    /// of failing (see [`TextView::from_bytes`]).
    pub fn show_loot_content(&mut self, key: &str, bytes: &[u8]) {
        self.text_view = Some(TextView::from_bytes(key, bytes));
        self.screen = Screen::TextView;
    }

    /// The fetch came back empty (the key vanished between listing and
    /// fetching, e.g. a wipe) — shows why instead of silently doing nothing,
    /// which would look like the button didn't register.
    pub fn show_loot_missing(&mut self, key: &str) {
        self.text_view = Some(TextView::from_bytes(key, b"(no longer available)"));
        self.screen = Screen::TextView;
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

    /// Route one resolved nav action.
    pub fn apply_nav(&mut self, action: NavAction) -> NavOutcome {
        match self.screen {
            // Any press wakes the menu up; the press itself isn't also
            // consumed as a menu action (surprising to move the cursor on
            // the very button press that opened the menu).
            Screen::Status => {
                self.screen = Screen::Menu;
                NavOutcome::None
            }
            Screen::Menu => match action {
                NavAction::Up => {
                    self.menu.move_up();
                    NavOutcome::None
                }
                NavAction::Down => {
                    self.menu.move_down();
                    NavOutcome::None
                }
                // A flat vertical list has no horizontal axis to move along —
                // Left/Right are only meaningful once a Form/TextView is open.
                NavAction::Left | NavAction::Right => NavOutcome::None,
                NavAction::Back => {
                    self.screen = Screen::Status;
                    NavOutcome::None
                }
                NavAction::Select => {
                    // A loot row: hand the key back for an async fetch
                    // rather than opening anything here (see `NavOutcome`).
                    if let Some(entry) = self.menu.selected_loot() {
                        return NavOutcome::FetchLoot(entry.key.clone());
                    }
                    // Param-less action: invoke it and switch to Status to
                    // watch it run.
                    if let Some(cmd) = self.menu.activate() {
                        self.screen = Screen::Status;
                        return NavOutcome::Invoke(cmd);
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
                    NavOutcome::None
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
                        NavAction::Left => {
                            form.left();
                            FormOutcome::Stay
                        }
                        NavAction::Right => {
                            form.right();
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
                    FormOutcome::Stay => NavOutcome::None,
                    FormOutcome::Submit(cmd) => {
                        self.form = None;
                        self.screen = Screen::Status; // watch it run
                        NavOutcome::Invoke(cmd)
                    }
                    FormOutcome::CloseToMenu => {
                        self.form = None;
                        self.screen = Screen::Menu;
                        NavOutcome::None
                    }
                }
            }
            Screen::TextView => {
                match action {
                    NavAction::Up => {
                        if let Some(tv) = self.text_view.as_mut() {
                            tv.line_up();
                        }
                    }
                    NavAction::Down => {
                        if let Some(tv) = self.text_view.as_mut() {
                            tv.line_down();
                        }
                    }
                    NavAction::Left => {
                        if let Some(tv) = self.text_view.as_mut() {
                            tv.page_up();
                        }
                    }
                    NavAction::Right => {
                        if let Some(tv) = self.text_view.as_mut() {
                            tv.page_down();
                        }
                    }
                    NavAction::Back => {
                        self.text_view = None;
                        self.screen = Screen::Menu;
                    }
                    NavAction::Select => {} // no meaning here
                }
                NavOutcome::None
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
        assert_eq!(app.apply_nav(NavAction::Down), NavOutcome::None);
        assert_eq!(app.screen(), Screen::Menu);
        assert_eq!(app.menu().selected(), 1, "the opening press shouldn't also move the cursor");
    }

    #[test]
    fn back_returns_to_status_without_invoking() {
        let mut app = App::new(&sample_manifest());
        app.apply_nav(NavAction::Down); // open menu
        assert_eq!(app.apply_nav(NavAction::Back), NavOutcome::None);
        assert_eq!(app.screen(), Screen::Status);
    }

    #[test]
    fn selecting_a_runnable_row_switches_back_to_status() {
        let mut app = App::new(&sample_manifest());
        app.apply_nav(NavAction::Down); // open menu, selected on net.ports scan
        app.apply_nav(NavAction::Down); // move to sys.info get (invokable)
        assert!(matches!(app.apply_nav(NavAction::Select), NavOutcome::Invoke(_)));
        assert_eq!(app.screen(), Screen::Status, "running an action returns to the live view");
    }

    #[test]
    fn selecting_a_spinnable_params_action_opens_a_form() {
        let mut app = App::new(&sample_manifest());
        app.apply_nav(NavAction::Down); // open menu, selected on net.ports scan (needs a host target)
        assert_eq!(
            app.apply_nav(NavAction::Select),
            NavOutcome::None,
            "opening a form doesn't invoke yet"
        );
        assert_eq!(app.screen(), Screen::Form);
        assert_eq!(app.form().unwrap().action(), "scan");
    }

    #[test]
    fn filling_a_host_form_and_submitting_invokes_with_the_value() {
        let mut app = App::new(&sample_manifest());
        app.apply_nav(NavAction::Down); // open menu on net.ports scan
        app.apply_nav(NavAction::Select); // open the form (host field, 4 octets)
        // Select walks octet 0->1->2->3, then the final Select submits.
        assert_eq!(app.apply_nav(NavAction::Select), NavOutcome::None);
        assert_eq!(app.apply_nav(NavAction::Select), NavOutcome::None);
        assert_eq!(app.apply_nav(NavAction::Select), NavOutcome::None);
        match app.apply_nav(NavAction::Select) {
            NavOutcome::Invoke(Command::Invoke(inv)) => {
                assert_eq!(inv.module, ModuleId::from("net.ports"));
                assert_eq!(inv.action, "scan");
                // No example in the sample spec -> host seeds at 0.0.0.0.
                assert_eq!(inv.params.0["target"], serde_json::json!("0.0.0.0"));
            }
            other => panic!("expected an invoke, got {other:?}"),
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
        assert_eq!(app.apply_nav(NavAction::Select), NavOutcome::None);
        assert_eq!(app.screen(), Screen::Menu, "browse-only action: Select is a no-op");
    }

    #[test]
    fn loot_rows_append_after_the_modules_and_are_navigable() {
        let mut menu = Menu::from_manifest(&sample_manifest());
        menu.set_loot(vec![LootEntry { key: "a".into(), kind: contract::LootKind::Other, size: 3 }]);
        // 4 rows from sample_manifest (2 groups + 2 modules), then Group("loot") + the entry.
        assert_eq!(menu.rows().len(), 6);
        assert!(matches!(&menu.rows()[4], Row::Group(g) if g == "loot"));
        assert!(matches!(&menu.rows()[5], Row::Loot(e) if e.key == "a"));

        // move_down from the last module row lands on the loot row (groups
        // are still skipped, but loot rows are now selectable like modules).
        menu.selected = 3; // sys.info get, the last module row
        menu.move_down();
        assert_eq!(menu.selected(), 5);
        assert!(menu.selected_loot().is_some());
    }

    #[test]
    fn note_loot_updates_an_existing_key_in_place_rather_than_duplicating() {
        let mut menu = Menu::from_manifest(&sample_manifest());
        menu.note_loot(LootEntry { key: "a".into(), kind: contract::LootKind::Other, size: 3 });
        menu.note_loot(LootEntry { key: "a".into(), kind: contract::LootKind::Other, size: 9 });
        let loot_rows: Vec<&LootEntry> =
            menu.rows().iter().filter_map(|r| match r { Row::Loot(e) => Some(e), _ => None }).collect();
        assert_eq!(loot_rows.len(), 1, "same key updates in place, doesn't duplicate");
        assert_eq!(loot_rows[0].size, 9);
    }

    fn loot_rows(menu: &Menu) -> Vec<&LootEntry> {
        menu.rows().iter().filter_map(|r| match r { Row::Loot(e) => Some(e), _ => None }).collect()
    }

    #[test]
    fn note_loot_inserts_new_entries_at_the_front() {
        let mut menu = Menu::from_manifest(&sample_manifest());
        menu.set_loot(vec![LootEntry { key: "old".into(), kind: contract::LootKind::Other, size: 1 }]);
        menu.note_loot(LootEntry { key: "new".into(), kind: contract::LootKind::Other, size: 2 });
        let keys: Vec<&str> = loot_rows(&menu).iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, vec!["new", "old"], "a brand-new item lands newest-first");
    }

    #[test]
    fn note_loot_caps_the_list_at_the_recent_limit() {
        let mut menu = Menu::from_manifest(&sample_manifest());
        for i in 0..crate::RECENT_LOOT_LIMIT + 5 {
            menu.note_loot(LootEntry { key: format!("k{i}"), kind: contract::LootKind::Other, size: 1 });
        }
        let rows = loot_rows(&menu);
        assert_eq!(rows.len(), crate::RECENT_LOOT_LIMIT, "old entries fall off once the cap is hit");
        assert_eq!(rows[0].key, format!("k{}", crate::RECENT_LOOT_LIMIT + 4));
    }

    #[test]
    fn cursor_snaps_to_a_valid_row_if_the_loot_section_it_was_on_disappears() {
        let mut menu = Menu::from_manifest(&sample_manifest());
        menu.set_loot(vec![LootEntry { key: "a".into(), kind: contract::LootKind::Other, size: 3 }]);
        menu.selected = 5; // the loot row
        menu.set_loot(vec![]); // loot cleared (e.g. a wipe) -- section disappears
        assert!(menu.selected() < menu.rows().len());
        assert!(!matches!(menu.rows().get(menu.selected()), Some(Row::Group(_))));
    }

    #[test]
    fn selecting_a_loot_row_requests_a_fetch_instead_of_invoking() {
        let mut app = App::new(&sample_manifest());
        app.set_loot_backlog(vec![LootEntry { key: "sysinfo/last".into(), kind: contract::LootKind::Telemetry, size: 3 }]);
        app.apply_nav(NavAction::Down); // open menu
        // Walk down to the loot row: net.ports scan, sys.info get, loot/last.
        app.apply_nav(NavAction::Down);
        app.apply_nav(NavAction::Down);
        match app.apply_nav(NavAction::Select) {
            NavOutcome::FetchLoot(key) => assert_eq!(key, "sysinfo/last"),
            other => panic!("expected FetchLoot, got {other:?}"),
        }
        // Fetching doesn't change the screen by itself -- only a successful
        // (or missing) result does, via show_loot_content/show_loot_missing.
        assert_eq!(app.screen(), Screen::Menu);
    }

    #[test]
    fn show_loot_content_opens_the_text_view() {
        let mut app = App::new(&sample_manifest());
        app.show_loot_content("k", b"hello\nworld");
        assert_eq!(app.screen(), Screen::TextView);
        assert_eq!(app.text_view().unwrap().lines(), &["hello", "world"]);
    }

    #[test]
    fn show_loot_missing_still_opens_a_view_explaining_why() {
        let mut app = App::new(&sample_manifest());
        app.show_loot_missing("k");
        assert_eq!(app.screen(), Screen::TextView);
        assert_eq!(app.text_view().unwrap().lines(), &["(no longer available)"]);
    }

    #[test]
    fn text_view_nav_scrolls_and_back_returns_to_the_menu() {
        let mut app = App::new(&sample_manifest());
        app.show_loot_content("k", b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10");
        assert_eq!(app.apply_nav(NavAction::Down), NavOutcome::None);
        assert_eq!(app.text_view().unwrap().scroll(), 1, "Down scrolls one line");
        app.apply_nav(NavAction::Right);
        assert_eq!(app.text_view().unwrap().scroll(), 5, "Right pages forward");
        app.apply_nav(NavAction::Back);
        assert_eq!(app.screen(), Screen::Menu);
        assert!(app.text_view().is_none(), "Back closes the view");
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
