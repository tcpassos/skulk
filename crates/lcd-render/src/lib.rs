//! On-device LCD renderer. An in-process consumer of the engine's event bus
//! (never a socket client — see [`engine::Engine::subscribe`]) that draws the
//! latest `Event::ViewManifest` via `embedded-graphics`, generic over the
//! physical display driver. Concrete drivers (mipidsi/rppal, ...) are thin
//! adapters added in later phases; this crate's core never depends on them.

mod driver_gpio_buttons;
mod driver_mipidsi;
mod hud;
mod input;
mod menu;
mod theme;

#[cfg(all(target_os = "linux", feature = "driver-mipidsi"))]
pub use driver_mipidsi::{
    open_st7735s, open_st7789, Config as MipidsiConfig, MipidsiDisplay,
};
#[cfg(all(target_os = "linux", feature = "input-gpio"))]
pub use driver_gpio_buttons::{ButtonConfig, GpioButtons};
pub use hud::{Hud, Slot};
pub use input::{InputEvent, InputSource, NavAction, NavMap};
pub use menu::{App, Menu, Row, Screen};
pub use theme::{decode_bmp, SeverityPalette, Theme, ThemeError};

use std::collections::HashMap;
use std::sync::Arc;

use contract::{Body, Envelope, Event, Manifest, Severity, ViewManifest};
use embedded_graphics::draw_target::{DrawTarget, DrawTargetExt};
use embedded_graphics::geometry::{OriginDimensions, Point, Size};
use embedded_graphics::image::Image;
use embedded_graphics::mono_font::ascii::FONT_5X8;
use embedded_graphics::mono_font::{MonoFont, MonoTextStyle, MonoTextStyleBuilder};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::primitives::Rectangle;
use embedded_graphics::text::Text;
use embedded_graphics::Drawable;
use engine::Engine;
use theme::FONT_CANDIDATES;
use tokio::sync::broadcast;

/// Compact fixed font for the HUD band, independent of the theme's content
/// font: the band must stay a thin strip no matter how large a theme sets
/// its main font.
const HUD_FONT: &MonoFont<'static> = &FONT_5X8;
/// Padding above+below the HUD text/icon, in pixels.
const HUD_PAD: u32 = 3;

/// Draws a [`ViewManifest`] onto any display using a [`Theme`]'s palette and
/// font, automatically shrinking the font to fit when the theme's preferred
/// size doesn't — the "automatic baseline" layout every theme gets for free.
#[derive(Default)]
pub struct Renderer {
    theme: Theme,
}

impl Renderer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_theme(theme: Theme) -> Self {
        Self { theme }
    }

    /// Draw one `ViewManifest`, replacing whatever was on `target` before.
    pub fn draw<D>(&self, target: &mut D, view: &ViewManifest) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        target.clear(self.theme.background)?;

        let bounds = target.bounding_box();
        let font = pick_font(Some(self.theme.font), bounds.size.height, view.lines.len() as u32 + 1);
        let header_style = MonoTextStyle::new(font, self.theme.foreground);
        let line_height = (font.character_size.height + font.character_spacing) as i32;

        let mut y = font.character_size.height as i32;
        Text::new(&view.screen, Point::new(0, y), header_style).draw(target)?;

        for line in &view.lines {
            y += line_height;
            if y > bounds.size.height as i32 {
                break; // overflow: v1 truncates, no scrolling/paging yet
            }
            let color = self.severity_color(line.severity);
            let style = MonoTextStyle::new(font, color);
            let text = format!("{}: {}", line.label, line.value);
            Text::new(&text, Point::new(0, y), style).draw(target)?;
        }
        Ok(())
    }

    fn severity_color(&self, severity: Option<Severity>) -> Rgb565 {
        match severity {
            None => self.theme.foreground,
            Some(Severity::Info) => self.theme.severity.info,
            Some(Severity::Low) => self.theme.severity.low,
            Some(Severity::Medium) => self.theme.severity.medium,
            Some(Severity::High) => self.theme.severity.high,
            Some(Severity::Critical) => self.theme.severity.critical,
        }
    }

    /// Draw the browsable module menu, replacing whatever was on `target`
    /// before. The selected row renders foreground/background-swapped —
    /// the only place this crate needs an inverted style, so it's built
    /// here rather than added as a `Theme` field.
    pub fn draw_menu<D>(&self, target: &mut D, menu: &Menu) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        target.clear(self.theme.background)?;

        let bounds = target.bounding_box();
        let rows = menu.rows();
        let font = pick_font(Some(self.theme.font), bounds.size.height, rows.len() as u32 + 1);
        let header_style = MonoTextStyle::new(font, self.theme.foreground);
        let selected_style = MonoTextStyleBuilder::new()
            .font(font)
            .text_color(self.theme.background)
            .background_color(self.theme.foreground)
            .build();
        let line_height = (font.character_size.height + font.character_spacing) as i32;

        let mut y = font.character_size.height as i32;
        Text::new("MENU", Point::new(0, y), header_style).draw(target)?;

        for (i, row) in rows.iter().enumerate() {
            y += line_height;
            if y > bounds.size.height as i32 {
                break; // overflow: v1 truncates, no scrolling/paging yet
            }
            let text = match row {
                Row::Group(name) => format!("{name}/"),
                Row::Module { id, action, invokable } => {
                    let name = id.0.split_once('.').map(|(_, n)| n).unwrap_or(&id.0);
                    let marker = if *invokable { "" } else { " *" };
                    format!(" {name} {action}{marker}")
                }
            };
            let style = if i == menu.selected() { selected_style } else { MonoTextStyle::new(font, self.theme.foreground) };
            Text::new(&text, Point::new(0, y), style).draw(target)?;
        }
        Ok(())
    }

    /// Draw whichever of the two screens `app` currently has active.
    pub fn draw_app<D>(&self, target: &mut D, app: &App) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        match app.screen() {
            Screen::Status => self.draw(target, &app.status_view()),
            Screen::Menu => self.draw_menu(target, app.menu()),
        }
    }

    /// Height in pixels the HUD band occupies (0 when disabled). Fixed from
    /// [`HUD_FONT`], so the strip stays thin regardless of the theme's font.
    pub fn hud_band_height(&self, hud: &Hud) -> u32 {
        if hud.is_disabled() {
            0
        } else {
            HUD_FONT.character_size.height + HUD_PAD * 2
        }
    }

    /// Preload the icon bytes for each declared HUD slot from the theme, once,
    /// so the per-frame draw doesn't re-read files. A slot with no matching
    /// theme asset simply gets no icon (text-only) — the graceful fallback
    /// when no theme is configured or a theme omits that icon.
    pub fn load_hud_icons(&self, slots: &[String]) -> HashMap<String, Vec<u8>> {
        slots
            .iter()
            .filter_map(|slot| self.theme.asset_bytes(slot).ok().map(|bytes| (slot.clone(), bytes)))
            .collect()
    }

    /// Draw the HUD band across the top strip: each visible slot as an
    /// optional icon (from `icons`) plus its value text, severity-tinted,
    /// laid out left to right until the width runs out.
    pub fn draw_hud<D>(
        &self,
        target: &mut D,
        hud: &Hud,
        icons: &HashMap<String, Vec<u8>>,
    ) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let band_height = self.hud_band_height(hud);
        if band_height == 0 {
            return Ok(());
        }
        let width = target.bounding_box().size.width;
        // The band's own background strip (may differ from content once themes
        // gain a dedicated HUD color; for now it reuses the theme background).
        target.fill_solid(
            &Rectangle::new(Point::zero(), Size::new(width, band_height)),
            self.theme.background,
        )?;

        let text_y = (HUD_PAD + HUD_FONT.character_size.height) as i32;
        let mut x = 1i32;
        for slot in hud.visible() {
            if x >= width as i32 {
                break; // ran out of strip; drop the rest (bounded band)
            }
            // Icon, if the theme supplied one for this slot.
            if let Some(bytes) = icons.get(&slot.name) {
                if let Ok(bmp) = decode_bmp(bytes) {
                    let icon_w = bmp.size().width as i32;
                    let icon_y = (band_height.saturating_sub(bmp.size().height) / 2) as i32;
                    Image::new(&bmp, Point::new(x, icon_y)).draw(target)?;
                    x += icon_w + 1;
                }
            }
            let color = self.severity_color(slot.severity);
            let style = MonoTextStyle::new(HUD_FONT, color);
            Text::new(&slot.value, Point::new(x, text_y), style).draw(target)?;
            x += slot.value.chars().count() as i32 * HUD_FONT.character_size.width as i32 + 4;
        }
        Ok(())
    }

    /// Full-frame render: the HUD band (if any) across the top, and `app`'s
    /// active screen in the region below it. The single entry point the
    /// [`run_app`] loop draws through.
    pub fn draw_frame<D>(
        &self,
        target: &mut D,
        app: &App,
        hud: &Hud,
        icons: &HashMap<String, Vec<u8>>,
    ) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let band = self.hud_band_height(hud);
        if band == 0 {
            return self.draw_app(target, app);
        }
        self.draw_hud(target, hud, icons)?;
        // Content renders into everything below the band. `cropped` reports the
        // reduced size to the content's font-fitting and offsets its origin, so
        // the existing screen code needs no awareness of the band.
        let size = target.bounding_box().size;
        let below = Rectangle::new(
            Point::new(0, band as i32),
            Size::new(size.width, size.height.saturating_sub(band)),
        );
        let mut content = target.cropped(&below);
        self.draw_app(&mut content, app)
    }

    /// Consume `Event::ViewManifest` off the engine's bus and draw each one
    /// as it arrives. Runs until the bus closes (engine shutdown).
    pub async fn run<D>(&self, mut rx: broadcast::Receiver<Envelope>, mut target: D)
    where
        D: DrawTarget<Color = Rgb565>,
    {
        loop {
            match rx.recv().await {
                Ok(env) => {
                    if let Body::Event(Event::ViewManifest(view)) = env.body {
                        if self.draw(&mut target, &view).is_err() {
                            tracing::warn!("lcd-render: draw failed");
                        }
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            }
        }
    }
}

/// Spawn a background task that draws `Event::ViewManifest`s from `rx` onto
/// `target` (the default [`Theme`]) until the bus closes. A real display's
/// draw calls block on SPI/I2C I/O, so this runs on a dedicated blocking
/// thread rather than the async runtime — `block_on` drives the one async
/// wait point (the channel recv) from inside that thread. Lets callers (e.g.
/// `skulkd`) spawn any concrete display type without depending on
/// `embedded-graphics` themselves just to name the bound.
pub fn spawn<D>(rx: broadcast::Receiver<Envelope>, target: D)
where
    D: DrawTarget<Color = Rgb565> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        tokio::runtime::Handle::current().block_on(Renderer::new().run(rx, target));
    });
}

/// The default `InputSource` for units with no `[[peripherals]]` wired: it
/// never produces an event, so [`run_app`]'s menu stays permanently
/// unreachable and the loop behaves exactly like [`run`] -- tactical view
/// only. Never returns rather than immediately yielding `None`, so it can't
/// be mistaken by the select loop for an exhausted/closed real source.
pub struct NoInput;

#[async_trait::async_trait]
impl InputSource for NoInput {
    async fn next_event(&mut self) -> Option<InputEvent> {
        std::future::pending().await
    }
}

/// Runs the full on-device experience: the tactical `ViewManifest` (as
/// [`run`] already does) plus a browsable/invokable menu built from
/// `manifest` and a composited HUD band of `hud_slots`, toggled by `input`
/// events resolved through `nav`. Takes `engine` itself, not just its bus --
/// activating a menu row sends a `Command::Invoke` back into it, the one
/// place this crate reaches beyond "draw what the bus already published".
/// `renderer` carries the theme (so `hud_slots`' icons resolve). Returns once
/// both the bus and `input` are exhausted (engine shutdown).
pub async fn run_app<D>(
    renderer: Renderer,
    engine: Arc<Engine>,
    manifest: &Manifest,
    mut input: Box<dyn InputSource>,
    nav: NavMap,
    hud_slots: Vec<String>,
    mut target: D,
) where
    D: DrawTarget<Color = Rgb565>,
{
    let mut app = App::new(manifest);
    let mut hud = Hud::new(hud_slots.clone());
    let icons = renderer.load_hud_icons(&hud_slots);
    let mut rx = engine.subscribe();

    // One place that turns current state into pixels, so every wake-up path
    // (view update, HUD update, input) redraws identically.
    let draw = |target: &mut D, app: &App, hud: &Hud| {
        if renderer.draw_frame(target, app, hud, &icons).is_err() {
            tracing::warn!("lcd-render: draw failed");
        }
    };

    draw(&mut target, &app, &hud);

    loop {
        tokio::select! {
            result = rx.recv() => {
                match result {
                    Ok(env) => match env.body {
                        Body::Event(Event::ViewManifest(view)) => {
                            app.apply_view(view);
                            // A background view update only matters on the
                            // tactical screen; don't disturb the menu.
                            if app.screen() == Screen::Status {
                                draw(&mut target, &app, &hud);
                            }
                        }
                        // A HUD slot shows over every screen, so always redraw
                        // (but only when the slot actually changed).
                        Body::Event(Event::Widget(update)) => {
                            if hud.apply(update) {
                                draw(&mut target, &app, &hud);
                            }
                        }
                        _ => {}
                    },
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            event = input.next_event() => {
                match event {
                    Some(InputEvent::Pressed(peripheral)) => {
                        match nav.resolve(&peripheral) {
                            Some(action) => {
                                if let Some(cmd) = app.apply_nav(action) {
                                    engine.handle(menu::envelope(cmd)).await;
                                }
                                draw(&mut target, &app, &hud);
                            }
                            None => tracing::info!(
                                peripheral,
                                "lcd: button press seen but has no [nav] binding"
                            ),
                        }
                    }
                    Some(_) => {} // Released/Rotated: no menu behaviour defined yet
                    None => break, // the input source is permanently exhausted
                }
            }
        }
    }
}

/// Spawn [`run_app`] on a dedicated blocking thread, for the same reason as
/// [`spawn`]: a real display's draw calls block on SPI/I2C I/O.
pub fn spawn_app<D>(
    renderer: Renderer,
    engine: Arc<Engine>,
    manifest: Manifest,
    input: Box<dyn InputSource>,
    nav: NavMap,
    hud_slots: Vec<String>,
    target: D,
) where
    D: DrawTarget<Color = Rgb565> + Send + 'static,
{
    tokio::task::spawn_blocking(move || {
        tokio::runtime::Handle::current()
            .block_on(run_app(renderer, engine, &manifest, input, nav, hud_slots, target));
    });
}

/// Starting from `preferred` (or the largest candidate if `None`), the
/// largest font — searching only at or below `preferred`'s size — whose line
/// height times `total_lines` fits within `available_height`. Never fails:
/// the smallest candidate is the final fallback, and [`Renderer::draw`]'s own
/// overflow guard truncates whatever still doesn't fit.
fn pick_font(
    preferred: Option<&'static MonoFont<'static>>,
    available_height: u32,
    total_lines: u32,
) -> &'static MonoFont<'static> {
    let start = preferred
        .and_then(|p| FONT_CANDIDATES.iter().position(|f| f.character_size == p.character_size))
        .unwrap_or(0);
    FONT_CANDIDATES[start..]
        .iter()
        .find(|f| f.character_size.height * total_lines <= available_height)
        .copied()
        .unwrap_or(FONT_CANDIDATES[FONT_CANDIDATES.len() - 1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::geometry::Size;
    use embedded_graphics::pixelcolor::RgbColor;
    use embedded_graphics_simulator::SimulatorDisplay;

    fn sample_view() -> ViewManifest {
        ViewManifest {
            screen: "net.ports".into(),
            lines: vec![
                contract::ViewLine { label: "TARGET".into(), value: "10.0.0.1".into(), severity: None },
                contract::ViewLine {
                    label: "OPEN".into(),
                    value: "3".into(),
                    severity: Some(Severity::Medium),
                },
            ],
        }
    }

    #[test]
    fn draws_non_background_pixels_on_a_small_display() {
        let mut display: SimulatorDisplay<Rgb565> = SimulatorDisplay::new(Size::new(135, 240));
        Renderer::new().draw(&mut display, &sample_view()).unwrap();

        let lit = (0..135)
            .flat_map(|x| (0..240).map(move |y| Point::new(x, y)))
            .any(|p| display.get_pixel(p) != Rgb565::BLACK);
        assert!(lit, "expected at least one foreground pixel to be drawn");
    }

    #[test]
    fn severity_line_uses_the_theme_severity_color() {
        let renderer = Renderer::new();
        let mut display: SimulatorDisplay<Rgb565> = SimulatorDisplay::new(Size::new(135, 240));
        renderer.draw(&mut display, &sample_view()).unwrap();

        let medium = renderer.theme.severity.medium;
        let has_medium_pixel =
            (0..135).flat_map(|x| (0..240).map(move |y| Point::new(x, y))).any(|p| display.get_pixel(p) == medium);
        assert!(has_medium_pixel, "expected the Medium-severity line to be drawn in the severity color");
    }

    #[test]
    fn pick_font_shrinks_to_fit_many_lines_on_a_tiny_display() {
        // A 64px-tall display can't fit 10 lines at the largest font (10x20 * 10 = 200px);
        // must fall back to something smaller.
        let font = pick_font(None, 64, 10);
        assert!(font.character_size.height <= 6, "expected a small font, got {:?}", font.character_size);
    }

    #[test]
    fn pick_font_prefers_largest_that_fits_when_unthemed() {
        let font = pick_font(None, 240, 2);
        assert_eq!(font.character_size, Size::new(10, 20));
    }

    #[test]
    fn pick_font_honours_a_theme_preference_that_fits() {
        // Plenty of room, but the theme asks for 6x10 specifically -- must
        // not silently upgrade to a larger font the theme didn't choose.
        let preferred = Theme::default().font; // 6x10
        let font = pick_font(Some(preferred), 240, 2);
        assert_eq!(font.character_size, Size::new(6, 10));
    }

    #[test]
    fn pick_font_shrinks_below_a_theme_preference_that_does_not_fit() {
        let preferred = &embedded_graphics::mono_font::ascii::FONT_10X20;
        let font = pick_font(Some(preferred), 30, 5); // 5 lines of 10x20 never fit in 30px
        assert!(font.character_size.height < 20, "expected a smaller fallback, got {:?}", font.character_size);
    }

    #[tokio::test]
    async fn run_returns_once_the_bus_closes() {
        let (tx, rx) = broadcast::channel(8);
        let display: SimulatorDisplay<Rgb565> = SimulatorDisplay::new(Size::new(135, 240));

        tx.send(Envelope::new(Body::Event(Event::ViewManifest(sample_view())), 0)).unwrap();
        drop(tx); // closes the channel once the one message is drained

        // If `run` mishandled `RecvError::Closed` this would hang forever;
        // the timeout turns that into a clean test failure instead.
        tokio::time::timeout(std::time::Duration::from_secs(2), Renderer::new().run(rx, display))
            .await
            .expect("run() should return once the channel closes, not hang");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn run_app_opens_the_menu_and_invokes_a_selection() {
        let implant = contract::ImplantInfo { id: "t".into(), hardware: "t".into(), firmware: "0".into() };
        let mut engine = Engine::new(implant, Vec::new(), std::sync::Arc::new(engine::MemLoot::default()));
        engine.register(std::sync::Arc::new(example_sysinfo::SysInfo));
        let engine = Arc::new(engine);
        let manifest = engine.manifest();
        let mut bus = engine.subscribe();

        // "btn_open" merely wakes the menu up (any resolved action does,
        // per App::apply_nav); "btn_select" then activates the only
        // invokable row -- sys.info's sole module, sole action.
        let nav = NavMap::new(
            &std::collections::HashMap::new(),
            &[("btn_open".to_string(), "down".to_string()), ("btn_select".to_string(), "select".to_string())]
                .into_iter()
                .collect(),
        );
        let input: Box<dyn InputSource> = Box::new(QueuedInput(
            [InputEvent::Pressed("btn_open".into()), InputEvent::Pressed("btn_select".into())].into(),
        ));
        let display: SimulatorDisplay<Rgb565> = SimulatorDisplay::new(Size::new(128, 128));

        // Returns once QueuedInput's third poll yields None -- proving the
        // Select press actually drove `apply_nav` rather than getting stuck.
        // Empty hud_slots: no band, exercises the menu/invoke path unchanged.
        run_app(Renderer::new(), engine, &manifest, input, nav, Vec::new(), display).await;

        let result = loop {
            match tokio::time::timeout(std::time::Duration::from_secs(5), bus.recv()).await {
                Ok(Ok(env)) => {
                    if let Body::Result(r) = env.body {
                        break r;
                    }
                }
                _ => panic!("no result received for the menu-triggered invoke"),
            }
        };
        assert_eq!(result.status, contract::TaskStatus::Ok);
    }

    fn tiny_manifest() -> Manifest {
        Manifest {
            protocol: 1,
            implant: contract::ImplantInfo { id: "t".into(), hardware: "t".into(), firmware: "0".into() },
            modules: vec![],
            capabilities: vec![],
            peripherals: vec![],
        }
    }

    #[test]
    fn draw_hud_renders_in_the_band_and_not_below_it() {
        let renderer = Renderer::new();
        let mut hud = Hud::new(vec!["battery".into()]);
        hud.apply(contract::WidgetUpdate {
            slot: "battery".into(),
            value: "42%".into(),
            severity: Some(Severity::Low),
        });
        let mut display: SimulatorDisplay<Rgb565> = SimulatorDisplay::new(Size::new(128, 128));
        renderer.draw_hud(&mut display, &hud, &HashMap::new()).unwrap();

        let band = renderer.hud_band_height(&hud) as i32;
        assert!(band > 0);
        let lit_in_band = (0..128)
            .flat_map(|x| (0..band).map(move |y| Point::new(x, y)))
            .any(|p| display.get_pixel(p) != Rgb565::BLACK);
        assert!(lit_in_band, "expected the slot's value text drawn in the band");
        let lit_below = (0..128)
            .flat_map(|x| (band..128).map(move |y| Point::new(x, y)))
            .any(|p| display.get_pixel(p) != Rgb565::BLACK);
        assert!(!lit_below, "draw_hud must confine itself to the band strip");
    }

    #[test]
    fn draw_frame_composites_hud_over_content_below_it() {
        let renderer = Renderer::new();
        let mut app = App::new(&tiny_manifest());
        app.apply_view(sample_view()); // give the status screen real content
        let mut hud = Hud::new(vec!["battery".into()]);
        hud.apply(contract::WidgetUpdate {
            slot: "battery".into(),
            value: "42%".into(),
            severity: Some(Severity::Low),
        });
        let mut display: SimulatorDisplay<Rgb565> = SimulatorDisplay::new(Size::new(128, 128));
        renderer.draw_frame(&mut display, &app, &hud, &HashMap::new()).unwrap();

        let band = renderer.hud_band_height(&hud) as i32;
        let lit_in_band = (0..128)
            .flat_map(|x| (0..band).map(move |y| Point::new(x, y)))
            .any(|p| display.get_pixel(p) != Rgb565::BLACK);
        let lit_below = (0..128)
            .flat_map(|x| (band..128).map(move |y| Point::new(x, y)))
            .any(|p| display.get_pixel(p) != Rgb565::BLACK);
        assert!(lit_in_band, "HUD band should have pixels");
        assert!(lit_below, "content should render below the band");
    }

    #[test]
    fn draw_frame_without_a_band_is_fullscreen_content() {
        // Empty HUD -> band height 0 -> identical to draw_app: content may
        // legitimately draw at the very top row.
        let renderer = Renderer::new();
        let mut app = App::new(&tiny_manifest());
        app.apply_view(sample_view());
        let hud = Hud::new(vec![]);
        assert_eq!(renderer.hud_band_height(&hud), 0);
        let mut display: SimulatorDisplay<Rgb565> = SimulatorDisplay::new(Size::new(128, 128));
        renderer.draw_frame(&mut display, &app, &hud, &HashMap::new()).unwrap();
        let lit = (0..128)
            .flat_map(|x| (0..128).map(move |y| Point::new(x, y)))
            .any(|p| display.get_pixel(p) != Rgb565::BLACK);
        assert!(lit, "content should render");
    }

    /// A hardware-free `InputSource` that replays a fixed event queue, then
    /// reports itself exhausted -- lets a test drive `run_app` to a clean,
    /// deterministic stop without any real GPIO.
    struct QueuedInput(std::collections::VecDeque<InputEvent>);

    #[async_trait::async_trait]
    impl InputSource for QueuedInput {
        async fn next_event(&mut self) -> Option<InputEvent> {
            self.0.pop_front()
        }
    }
}
