//! On-device LCD renderer. An in-process consumer of the engine's event bus
//! (never a socket client — see [`engine::Engine::subscribe`]) that draws the
//! latest `Event::ViewManifest` via `embedded-graphics`, generic over the
//! physical display driver. Concrete drivers (mipidsi/rppal, ...) are thin
//! adapters added in later phases; this crate's core never depends on them.

mod driver_gpio_buttons;
mod driver_mipidsi;
mod input;
mod theme;

#[cfg(all(target_os = "linux", feature = "driver-mipidsi"))]
pub use driver_mipidsi::{open as open_mipidsi, Config as MipidsiConfig, St7789Display};
#[cfg(all(target_os = "linux", feature = "input-gpio"))]
pub use driver_gpio_buttons::{ButtonConfig, GpioButtons};
pub use input::{InputEvent, InputSource, NavAction, NavMap};
pub use theme::{decode_bmp, SeverityPalette, Theme, ThemeError};

use contract::{Body, Envelope, Event, Severity, ViewManifest};
use embedded_graphics::draw_target::DrawTarget;
use embedded_graphics::geometry::Point;
use embedded_graphics::mono_font::{MonoFont, MonoTextStyle};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::text::Text;
use embedded_graphics::Drawable;
use theme::FONT_CANDIDATES;
use tokio::sync::broadcast;

/// Draws a [`ViewManifest`] onto any display using a [`Theme`]'s palette and
/// font, automatically shrinking the font to fit when the theme's preferred
/// size doesn't — the "automatic baseline" layout every theme gets for free.
pub struct Renderer {
    theme: Theme,
}

impl Default for Renderer {
    fn default() -> Self {
        Self { theme: Theme::default() }
    }
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
}
