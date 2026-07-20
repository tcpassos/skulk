//! MIPI-DCS panels over SPI via `rppal`, through `mipidsi`. Linux-only —
//! `rppal` talks to `/dev/spidev*`/`/dev/gpiochip*`, which don't exist
//! elsewhere — so this whole module compiles to nothing unless both the
//! platform AND the opt-in `driver-mipidsi` feature (which is what actually
//! makes `mipidsi`/`rppal` available as dependencies) are present, matching
//! the split already used in `skulkd::caps::detect`.
#![cfg(all(target_os = "linux", feature = "driver-mipidsi"))]

use std::thread;
use std::time::Duration;

use embedded_hal::delay::DelayNs;
use mipidsi::interface::SpiInterface;
use mipidsi::models::{Model, ST7735s, ST7789};
use mipidsi::options::{ColorOrder, Orientation, Rotation};
use mipidsi::{Builder, Display};
use rppal::gpio::{Gpio, OutputPin};
use rppal::spi::{Bus, Mode, SimpleHalSpiDevice, SlaveSelect, Spi};

/// Physical wiring for one display — field-for-field what
/// `skulkd::config::DisplaySection` carries, kept as a separate type so this
/// crate doesn't depend on skulkd's config structs.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Passed straight through to `mipidsi::Builder::display_size()`, which
    /// wants the panel's NATIVE (as-if-`rotation`-were-0) dimensions, not
    /// the final on-screen shape. A panel that's natively portrait but used
    /// here in landscape (e.g. the Waveshare 1.14", native 135x240) needs
    /// width/height set to that native 135x240, not the rotated 240x135 —
    /// getting this backwards is what causes a stray/garbage line on one
    /// edge and a clipped line on the opposite edge, since `mipidsi` then
    /// reads outside the real glass window.
    pub width: u16,
    pub height: u16,
    /// Offset of the visible panel within the controller's larger
    /// addressable framebuffer, in that SAME native frame as width/height
    /// above. Given a native-frame offset, `mipidsi` re-derives the
    /// effective per-rotation offset for you, so the same values keep
    /// working across every rotation — but only once width/height/offset
    /// are all in that native frame; feeding it final-on-screen dimensions
    /// instead defeats this and offsets stop carrying over between rotations.
    pub offset_x: u16,
    pub offset_y: u16,
    /// Clockwise rotation in degrees: 0, 90, 180, or 270. For a natively
    /// portrait panel used in landscape, only 90/270 actually swap the axes
    /// into landscape — 0/180 stay portrait.
    pub rotation: u16,
    pub spi_bus: u8,
    pub spi_cs: u8,
    pub dc_gpio: u8,
    pub rst_gpio: u8,
    pub bl_gpio: u8,
    /// Panel subpixel order. `mipidsi` defaults to RGB; several common small
    /// boards (e.g. the Waveshare 1.44" LCD HAT / ST7735S) actually need
    /// BGR, or colors come out swapped.
    pub bgr: bool,
}

pub type MipidsiDisplay<MODEL> =
    Display<SpiInterface<'static, SimpleHalSpiDevice, OutputPin>, MODEL, OutputPin>;

/// Waveshare's bare 1.14" LCD Module (this project's first target).
pub fn open_st7789(config: &Config) -> Result<MipidsiDisplay<ST7789>, String> {
    open(config, ST7789)
}

/// Waveshare's 1.44" LCD HAT — same pin conventions as the 1.14" module,
/// different chip.
pub fn open_st7735s(config: &Config) -> Result<MipidsiDisplay<ST7735s>, String> {
    open(config, ST7735s)
}

/// Open the physical display per `config`. The backlight is driven high only
/// after `init` succeeds, so a failed init doesn't flash garbage on screen.
fn open<MODEL>(config: &Config, model: MODEL) -> Result<MipidsiDisplay<MODEL>, String>
where
    MODEL: Model<ColorFormat = embedded_graphics::pixelcolor::Rgb565>,
{
    let bus = match config.spi_bus {
        0 => Bus::Spi0,
        1 => Bus::Spi1,
        other => return Err(format!("unsupported spi_bus {other}, expected 0 or 1")),
    };
    let slave_select = match config.spi_cs {
        0 => SlaveSelect::Ss0,
        1 => SlaveSelect::Ss1,
        other => return Err(format!("unsupported spi_cs {other}, expected 0 or 1")),
    };
    let rotation = match config.rotation {
        0 => Rotation::Deg0,
        90 => Rotation::Deg90,
        180 => Rotation::Deg180,
        270 => Rotation::Deg270,
        other => return Err(format!("unsupported rotation {other}, expected 0, 90, 180, or 270")),
    };
    // 9 MHz: a conservative starting clock for ST7735S/ST7789 over a short
    // ribbon. Raise it once the picture is confirmed stable; lower it first
    // if pixels come out garbled.
    let spi = Spi::new(bus, slave_select, 9_000_000, Mode::Mode0)
        .map_err(|e| format!("cannot open SPI bus {}: {e}", config.spi_bus))?;
    let spi_device = SimpleHalSpiDevice::new(spi);

    let gpio = Gpio::new().map_err(|e| format!("cannot open gpiochip: {e}"))?;
    let dc = gpio
        .get(config.dc_gpio)
        .map_err(|e| format!("dc_gpio {}: {e}", config.dc_gpio))?
        .into_output();
    let rst = gpio
        .get(config.rst_gpio)
        .map_err(|e| format!("rst_gpio {}: {e}", config.rst_gpio))?
        .into_output();
    let mut backlight = gpio
        .get(config.bl_gpio)
        .map_err(|e| format!("bl_gpio {}: {e}", config.bl_gpio))?
        .into_output();
    backlight.set_low();

    // `SpiInterface` needs a scratch buffer for chunked pixel writes, sized
    // well under one full frame on purpose — it's a staging buffer, not a
    // framebuffer. Leaked deliberately: the display (and this buffer) live
    // for the daemon's whole run, so there's nowhere shorter-lived to own it
    // without extra indirection.
    let buffer: &'static mut [u8] = Box::leak(Box::new([0u8; 4096]));
    let interface = SpiInterface::new(spi_device, dc, buffer);

    let color_order = if config.bgr { ColorOrder::Bgr } else { ColorOrder::Rgb };
    let mut delay = StdDelay;
    let display = Builder::new(model, interface)
        .reset_pin(rst)
        .display_size(config.width, config.height)
        .display_offset(config.offset_x, config.offset_y)
        .orientation(Orientation::new().rotate(rotation))
        .color_order(color_order)
        .init(&mut delay)
        .map_err(|e| format!("display init failed: {e:?}"))?;

    backlight.set_high();
    Ok(display)
}

/// `mipidsi::Builder::init` needs a `DelayNs`; this project runs on `std`
/// (not bare-metal), so a plain thread sleep is the simplest correct impl.
struct StdDelay;

impl DelayNs for StdDelay {
    fn delay_ns(&mut self, ns: u32) {
        thread::sleep(Duration::from_nanos(u64::from(ns)));
    }
}
