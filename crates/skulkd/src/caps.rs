//! Best-effort runtime capability detection. On Linux it probes filesystem
//! markers; on other platforms it reports nothing. Coarse by design — precise
//! per-radio probing (nl80211 monitor-mode support, etc.) is left to the modules
//! that need it, which fail cleanly via capability-gating if it is absent.

use contract::Capability;

#[cfg(target_os = "linux")]
pub fn detect() -> Vec<Capability> {
    let mut caps = Vec::new();
    // A USB Device Controller present means the board can act as a USB gadget.
    if dir_has_entries("/sys/class/udc") {
        caps.push(Capability::UsbGadget);
    }
    // A Bluetooth controller registered with the kernel.
    if dir_has_entries("/sys/class/bluetooth") {
        caps.push(Capability::Bluetooth);
    }
    // At least one I2C adapter registered (e.g. the Pi's onboard bus 1) --
    // needed by I2C-wired sensors like `sys.battery`'s INA219.
    if dir_has_entries("/sys/class/i2c-dev") {
        caps.push(Capability::I2c);
    }
    caps
}

#[cfg(target_os = "linux")]
fn dir_has_entries(path: &str) -> bool {
    std::fs::read_dir(path).map(|mut d| d.next().is_some()).unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
pub fn detect() -> Vec<Capability> {
    // Non-Linux dev hosts expose none of the implant hardware.
    Vec::new()
}
