//! ST7789 (and other MIPI-DCS panels `mipidsi` supports) over SPI via
//! `rppal`. Linux-only — `rppal` talks to `/dev/spidev*`/`/dev/gpiochip*`,
//! which don't exist elsewhere — so this whole module compiles to nothing
//! unless both the platform AND the opt-in `driver-mipidsi` feature (which
//! is what actually makes `mipidsi`/`rppal` available as dependencies) are
//! present, matching the split already used in `skulkd::caps::detect`.
#![cfg(all(target_os = "linux", feature = "driver-mipidsi"))]

use std::thread;
use std::time::Duration;

use embedded_hal::delay::DelayNs;
use mipidsi::interface::SpiInterface;
use mipidsi::models::ST7789;
use mipidsi::{Builder, Display};
use rppal::gpio::{Gpio, OutputPin};
use rppal::spi::{Bus, Mode, SimpleHalSpiDevice, SlaveSelect, Spi};

/// Physical wiring for one display — field-for-field what
/// `skulkd::config::DisplaySection` carries, kept as a separate type so this
/// crate doesn't depend on skulkd's config structs.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub width: u16,
    pub height: u16,
    /// Offset of the visible panel within the ST7789 controller's larger
    /// addressable framebuffer (up to 240x320) — most small ST7789 boards,
    /// including the Waveshare 1.14" this was built against, need a nonzero
    /// offset or the image is shifted/cropped. Start at (0, 0) and adjust
    /// against the real display if the picture looks off.
    pub offset_x: u16,
    pub offset_y: u16,
    pub spi_bus: u8,
    pub spi_cs: u8,
    pub dc_gpio: u8,
    pub rst_gpio: u8,
    pub bl_gpio: u8,
}

pub type St7789Display =
    Display<SpiInterface<'static, SimpleHalSpiDevice, OutputPin>, ST7789, OutputPin>;

/// Open the physical display per `config`. The backlight is driven high only
/// after `init` succeeds, so a failed init doesn't flash garbage on screen.
pub fn open(config: &Config) -> Result<St7789Display, String> {
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
    // 32 MHz: a common safe default for ST7789 over a short ribbon; slow
    // this down first if the real display shows garbled pixels.
    let spi = Spi::new(bus, slave_select, 32_000_000, Mode::Mode0)
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
    // well under one full frame (135x240 RGB565 = 64_800 B) on purpose —
    // it's a staging buffer, not a framebuffer. Leaked deliberately: the
    // display (and this buffer) live for the daemon's whole run, so there's
    // nowhere shorter-lived to own it without extra indirection.
    let buffer: &'static mut [u8] = Box::leak(Box::new([0u8; 4096]));
    let interface = SpiInterface::new(spi_device, dc, buffer);

    let mut delay = StdDelay;
    let display = Builder::new(ST7789, interface)
        .reset_pin(rst)
        .display_size(config.width, config.height)
        .display_offset(config.offset_x, config.offset_y)
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
