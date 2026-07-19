//! Discrete GPIO buttons over `rppal`, wired up as pull-up inputs (button
//! press pulls the pin low — the common wiring for these small boards).
//! Linux-only, same reasoning as `driver_mipidsi`.
#![cfg(all(target_os = "linux", feature = "input-gpio"))]

use std::time::Duration;

use rppal::gpio::{Gpio, InputPin, Trigger};
use tokio::sync::mpsc;

use crate::input::{InputEvent, InputSource};

/// One wired button: logical name (matches `skulk.toml`'s `[[peripherals]]`
/// and a theme's `[nav]`) plus its BCM GPIO number.
#[derive(Debug, Clone)]
pub struct ButtonConfig {
    pub name: String,
    pub gpio: u8,
}

/// Reads a set of buttons via GPIO interrupts and turns edges into
/// [`InputEvent`]s on an internal channel that [`InputSource::next_event`]
/// drains.
pub struct GpioButtons {
    rx: mpsc::UnboundedReceiver<InputEvent>,
    // Held only to keep each pin's interrupt registration alive — rppal
    // clears it when the `InputPin` drops.
    _pins: Vec<InputPin>,
}

impl GpioButtons {
    /// Debounce window applied to every button; short enough to feel
    /// instant, long enough to swallow typical mechanical-switch bounce.
    const DEBOUNCE: Duration = Duration::from_millis(20);

    pub fn open(buttons: &[ButtonConfig]) -> Result<Self, String> {
        let gpio = Gpio::new().map_err(|e| format!("cannot open gpiochip: {e}"))?;
        let (tx, rx) = mpsc::unbounded_channel();

        let mut pins = Vec::with_capacity(buttons.len());
        for button in buttons {
            let mut pin = gpio
                .get(button.gpio)
                .map_err(|e| format!("{} (gpio {}): {e}", button.name, button.gpio))?
                .into_input_pullup();
            let name = button.name.clone();
            let tx = tx.clone();
            pin.set_async_interrupt(Trigger::Both, Some(Self::DEBOUNCE), move |event| {
                let ev = match event.trigger {
                    Trigger::FallingEdge => InputEvent::Pressed(name.clone()),
                    _ => InputEvent::Released(name.clone()),
                };
                // The receiver may already be gone (renderer shut down);
                // nothing to do about a send failure from an interrupt callback.
                let _ = tx.send(ev);
            })
            .map_err(|e| format!("{} (gpio {}): {e}", button.name, button.gpio))?;
            pins.push(pin);
        }

        Ok(Self { rx, _pins: pins })
    }
}

#[async_trait::async_trait]
impl InputSource for GpioButtons {
    async fn next_event(&mut self) -> Option<InputEvent> {
        self.rx.recv().await
    }
}
