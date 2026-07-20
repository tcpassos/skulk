//! Minimal INA219 current/voltage sensor driver over I2C — just enough to
//! read bus voltage and current for a battery gauge. Linux-only (`rppal`);
//! every other platform gets a stub that fails cleanly at invoke time (the
//! engine's capability-gating on `Capability::I2c` means the real engine
//! dispatch path never actually reaches it there), so this crate still
//! builds on the Windows dev machine (see CLAUDE.md).
//!
//! Register layout and calibration constants match this repo's RaspyJack
//! fork's own INA219 driver (`plugins/battery_status_plugin/_impl.py`),
//! proven against the same physical sensor.

use module_sdk::ModuleError;

#[cfg(target_os = "linux")]
const REG_CONFIG: u8 = 0x00;
#[cfg(target_os = "linux")]
const REG_BUS_VOLTAGE: u8 = 0x02;
#[cfg(target_os = "linux")]
const REG_CURRENT: u8 = 0x04;
#[cfg(target_os = "linux")]
const REG_CALIBRATION: u8 = 0x05;

/// Calibration for a 32V/2A range — this project's reference hardware.
#[cfg(target_os = "linux")]
const CAL_VALUE: u16 = 26868;
#[cfg(target_os = "linux")]
const CURRENT_LSB_MA: f64 = 0.1524;

/// Reads `(bus_voltage_v, current_a)` from the INA219 at `address` on I2C
/// `bus`. Re-does calibration/config on every call rather than keeping a
/// persistent handle — a few extra I2C transactions is nothing next to the
/// multi-second polling interval `sys.battery watch` uses, and it avoids any
/// sensor-handle lifecycle to manage.
#[cfg(target_os = "linux")]
pub fn read(bus: u8, address: u16) -> Result<(f64, f64), ModuleError> {
    use rppal::i2c::I2c;

    let mut i2c = I2c::with_bus(bus).map_err(|e| ModuleError::Failed(format!("cannot open I2C bus {bus}: {e}")))?;
    i2c.set_slave_address(address)
        .map_err(|e| ModuleError::Failed(format!("cannot address INA219 at {address:#x}: {e}")))?;

    // 32V range, /8 gain, 12-bit ADC, continuous shunt+bus sampling.
    let config: u16 = (0x01 << 11) | (0x0D << 7) | (0x0D << 3) | 0x07;
    i2c.smbus_write_word_swapped(REG_CALIBRATION, CAL_VALUE)
        .map_err(|e| ModuleError::Failed(format!("INA219 calibration write failed: {e}")))?;
    i2c.smbus_write_word_swapped(REG_CONFIG, config)
        .map_err(|e| ModuleError::Failed(format!("INA219 config write failed: {e}")))?;

    // The first bus-voltage read after a config/calibration write can return
    // a stale conversion; discard it and read again, matching the reference
    // driver.
    let _discard = i2c
        .smbus_read_word_swapped(REG_BUS_VOLTAGE)
        .map_err(|e| ModuleError::Failed(format!("INA219 bus voltage read failed: {e}")))?;
    let raw_voltage = i2c
        .smbus_read_word_swapped(REG_BUS_VOLTAGE)
        .map_err(|e| ModuleError::Failed(format!("INA219 bus voltage read failed: {e}")))?;
    let voltage = ((raw_voltage >> 3) as f64) * 0.004;

    let raw_current = i2c
        .smbus_read_word_swapped(REG_CURRENT)
        .map_err(|e| ModuleError::Failed(format!("INA219 current read failed: {e}")))?;
    // The current register is a signed 16-bit two's-complement value.
    let current = ((raw_current as i16) as f64 * CURRENT_LSB_MA) / 1000.0;

    Ok((voltage, current))
}

#[cfg(not(target_os = "linux"))]
pub fn read(_bus: u8, _address: u16) -> Result<(f64, f64), ModuleError> {
    Err(ModuleError::Unsupported("I2C is only available on Linux".to_string()))
}
