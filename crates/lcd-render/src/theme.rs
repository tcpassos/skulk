//! Community themes: a folder with a `theme.toml` (palette, font, optional
//! nav map) plus `.bmp` assets, loaded at runtime — no recompiling Rust.
//!
//! Parsing is deliberately lenient (no `deny_unknown_fields`, unlike
//! `skulkd`'s operator-local config): a theme is community-authored and
//! evolving, so an unrecognized field (e.g. a future per-resolution
//! `[[variant]]` table) is ignored rather than rejecting the whole theme.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use embedded_graphics::mono_font::ascii::{
    FONT_10X20, FONT_4X6, FONT_5X8, FONT_6X10, FONT_7X13, FONT_8X13, FONT_9X18,
};
use embedded_graphics::mono_font::MonoFont;
use embedded_graphics::pixelcolor::{Rgb565, Rgb888, RgbColor};
use serde::Deserialize;
use tinybmp::Bmp;

/// Largest-to-smallest, so [`crate::pick_font`] can search downward from a
/// preferred size. The name in [`FontSection`] is one of these, lowercased
/// (`"6x10"`, `"10x20"`, ...).
pub(crate) const FONT_CANDIDATES: &[&MonoFont<'static>] =
    &[&FONT_10X20, &FONT_9X18, &FONT_8X13, &FONT_7X13, &FONT_6X10, &FONT_5X8, &FONT_4X6];

/// A theme's resolved runtime palette + font — what [`crate::Renderer`] draws
/// with. Falls back to sane built-in defaults for anything a `theme.toml`
/// omits.
#[derive(Debug, Clone)]
pub struct Theme {
    pub background: Rgb565,
    pub foreground: Rgb565,
    pub severity: SeverityPalette,
    pub font: &'static MonoFont<'static>,
    /// Peripheral name -> logical nav action name, as declared by the theme.
    /// Kept as raw strings: the real `NavAction` enum lives in the input
    /// layer (a later phase), which resolves/validates these against the
    /// device's actual wired peripherals.
    pub nav: HashMap<String, String>,
    /// Asset name -> path, relative to the theme's directory (e.g. `"idle"
    /// -> "idle.bmp"`). Resolved lazily by [`Theme::asset_bytes`].
    assets: HashMap<String, String>,
    directory: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy)]
pub struct SeverityPalette {
    pub info: Rgb565,
    pub low: Rgb565,
    pub medium: Rgb565,
    pub high: Rgb565,
    pub critical: Rgb565,
}

impl Default for SeverityPalette {
    fn default() -> Self {
        Self {
            info: hex("#4a9eff").unwrap(),
            low: hex("#8888aa").unwrap(),
            medium: hex("#e0c040").unwrap(),
            high: hex("#ff8040").unwrap(),
            critical: hex("#ff3030").unwrap(),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            background: Rgb565::BLACK,
            foreground: Rgb565::WHITE,
            severity: SeverityPalette::default(),
            font: &FONT_6X10,
            nav: HashMap::new(),
            assets: HashMap::new(),
            directory: None,
        }
    }
}

impl Theme {
    /// Load a theme from a directory containing `theme.toml`. Any field the
    /// file omits keeps the built-in default.
    pub fn load(dir: impl AsRef<Path>) -> Result<Self, ThemeError> {
        let dir = dir.as_ref();
        let text = fs::read_to_string(dir.join("theme.toml"))
            .map_err(|e| ThemeError::Io(dir.join("theme.toml"), e.to_string()))?;
        let file: ThemeFile = toml::from_str(&text).map_err(|e| ThemeError::Parse(e.to_string()))?;
        Self::from_file(file, Some(dir.to_path_buf()))
    }

    fn from_file(file: ThemeFile, directory: Option<PathBuf>) -> Result<Self, ThemeError> {
        let defaults = Theme::default();
        let background = match &file.palette.background {
            Some(s) => hex(s)?,
            None => defaults.background,
        };
        let foreground = match &file.palette.foreground {
            Some(s) => hex(s)?,
            None => defaults.foreground,
        };
        let severity = SeverityPalette {
            info: opt_hex(&file.palette.severity_info, defaults.severity.info)?,
            low: opt_hex(&file.palette.severity_low, defaults.severity.low)?,
            medium: opt_hex(&file.palette.severity_medium, defaults.severity.medium)?,
            high: opt_hex(&file.palette.severity_high, defaults.severity.high)?,
            critical: opt_hex(&file.palette.severity_critical, defaults.severity.critical)?,
        };
        let font = match &file.font.name {
            Some(name) => font_by_name(name).ok_or_else(|| ThemeError::UnknownFont(name.clone()))?,
            None => defaults.font,
        };
        Ok(Theme { background, foreground, severity, font, nav: file.nav, assets: file.assets, directory })
    }

    /// Read one declared asset's raw bytes, relative to the theme's own
    /// directory. Decoding (e.g. via [`decode_bmp`]) is a separate step —
    /// callers only pay for it if they actually draw the asset.
    pub fn asset_bytes(&self, name: &str) -> Result<Vec<u8>, ThemeError> {
        let dir = self.directory.as_ref().ok_or(ThemeError::NoDirectory)?;
        let file = self.assets.get(name).ok_or_else(|| ThemeError::UnknownAsset(name.to_string()))?;
        let path = dir.join(file);
        fs::read(&path).map_err(|e| ThemeError::Io(path, e.to_string()))
    }
}

/// Decode BMP bytes (as loaded via [`Theme::asset_bytes`]) into a drawable
/// image. Community themes ship `.bmp` sprites/icons because it's a format
/// any image editor can export — no bespoke asset tooling needed.
pub fn decode_bmp(bytes: &[u8]) -> Result<Bmp<'_, Rgb565>, ThemeError> {
    Bmp::from_slice(bytes).map_err(|e| ThemeError::Bmp(format!("{e:?}")))
}

fn font_by_name(name: &str) -> Option<&'static MonoFont<'static>> {
    let name = name.to_ascii_lowercase();
    FONT_CANDIDATES
        .iter()
        .find(|f| format!("{}x{}", f.character_size.width, f.character_size.height) == name)
        .copied()
}

fn opt_hex(value: &Option<String>, default: Rgb565) -> Result<Rgb565, ThemeError> {
    match value {
        Some(s) => hex(s),
        None => Ok(default),
    }
}

/// Parse `"#rrggbb"` into `Rgb565`, going through `Rgb888` (whose 8-bit
/// channels match hex literally) and the crate's own tested `Rgb888 ->
/// Rgb565` conversion rather than hand-rolling bit-scaling.
fn hex(s: &str) -> Result<Rgb565, ThemeError> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 {
        return Err(ThemeError::BadColor(s.to_string()));
    }
    let byte = |i: usize| -> Result<u8, ThemeError> {
        u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ThemeError::BadColor(s.to_string()))
    };
    let (r, g, b) = (byte(0)?, byte(2)?, byte(4)?);
    Ok(Rgb888::new(r, g, b).into())
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ThemeFile {
    palette: PaletteSection,
    font: FontSection,
    assets: HashMap<String, String>,
    nav: HashMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct PaletteSection {
    background: Option<String>,
    foreground: Option<String>,
    severity_info: Option<String>,
    severity_low: Option<String>,
    severity_medium: Option<String>,
    severity_high: Option<String>,
    severity_critical: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct FontSection {
    name: Option<String>,
}

#[derive(Debug)]
pub enum ThemeError {
    Io(PathBuf, String),
    Parse(String),
    BadColor(String),
    UnknownFont(String),
    Bmp(String),
    NoDirectory,
    UnknownAsset(String),
}

impl std::fmt::Display for ThemeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThemeError::Io(path, e) => write!(f, "cannot read {}: {e}", path.display()),
            ThemeError::Parse(e) => write!(f, "invalid theme.toml: {e}"),
            ThemeError::BadColor(s) => write!(f, "bad color '#{s}', expected #rrggbb"),
            ThemeError::UnknownFont(s) => write!(f, "unknown font '{s}'"),
            ThemeError::Bmp(e) => write!(f, "invalid bmp asset: {e}"),
            ThemeError::NoDirectory => write!(f, "theme has no directory (built with Theme::default())"),
            ThemeError::UnknownAsset(name) => write!(f, "theme has no asset named '{name}'"),
        }
    }
}

impl std::error::Error for ThemeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::geometry::OriginDimensions;

    const CHESSBOARD_BMP: &[u8] = include_bytes!("../tests/fixtures/chessboard-4px-rgb565.bmp");

    #[test]
    fn full_theme_toml_parses() {
        // Uses `r##"..."##`: the TOML content's own `"#rrggbb"` strings
        // would otherwise prematurely close a single-`#` raw string.
        let text = r##"
            [palette]
            background = "#0a0a0a"
            foreground = "#e0e0e0"
            severity_high = "#ff8040"

            [font]
            name = "6X10"

            [assets]
            idle = "idle.bmp"

            [nav]
            btn_a = "up"
            btn_b = "down"
        "##;
        let file: ThemeFile = toml::from_str(text).unwrap();
        let theme = Theme::from_file(file, None).unwrap();

        assert_eq!(theme.background, hex("#0a0a0a").unwrap());
        assert_eq!(theme.foreground, hex("#e0e0e0").unwrap());
        assert_eq!(theme.severity.high, hex("#ff8040").unwrap());
        // Untouched severity colors keep the built-in default.
        assert_eq!(theme.severity.info, SeverityPalette::default().info);
        assert_eq!(theme.font.character_size, embedded_graphics::geometry::Size::new(6, 10));
        assert_eq!(theme.nav.get("btn_a"), Some(&"up".to_string()));
        assert_eq!(theme.assets.get("idle"), Some(&"idle.bmp".to_string()));
    }

    #[test]
    fn empty_theme_toml_uses_all_defaults() {
        let file: ThemeFile = toml::from_str("").unwrap();
        let theme = Theme::from_file(file, None).unwrap();
        let defaults = Theme::default();
        assert_eq!(theme.background, defaults.background);
        assert_eq!(theme.foreground, defaults.foreground);
        assert_eq!(theme.font.character_size, defaults.font.character_size);
    }

    #[test]
    fn unrecognized_fields_are_ignored_not_rejected() {
        // Community themes must survive fields from a newer/future version
        // (e.g. a not-yet-implemented `[[variant]]` table) without erroring.
        let text = r##"
            [palette]
            background = "#000000"

            [[variant]]
            min_width = 240
        "##;
        let file: Result<ThemeFile, _> = toml::from_str(text);
        assert!(file.is_ok(), "unknown sections must not fail parsing: {file:?}");
    }

    #[test]
    fn unknown_font_name_errors() {
        let text = r#"[font]
name = "99x99""#;
        let file: ThemeFile = toml::from_str(text).unwrap();
        assert!(matches!(Theme::from_file(file, None), Err(ThemeError::UnknownFont(_))));
    }

    #[test]
    fn bad_color_errors() {
        assert!(matches!(hex("not-a-color"), Err(ThemeError::BadColor(_))));
        assert!(matches!(hex("#zzzzzz"), Err(ThemeError::BadColor(_))));
    }

    #[test]
    fn decodes_a_real_bmp_asset() {
        // A tiny fixture borrowed from tinybmp's own test suite — proves the
        // community-facing asset pipeline actually decodes a real file, not
        // just that the types line up.
        let bmp = decode_bmp(CHESSBOARD_BMP).expect("valid rgb565 bmp must decode");
        assert_eq!(bmp.size(), embedded_graphics::geometry::Size::new(4, 4));
    }

    #[test]
    fn theme_load_reads_toml_and_resolves_asset_bytes() {
        let dir = std::env::temp_dir().join(format!("skulk-lcd-theme-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("theme.toml"), "[assets]\nsprite = \"sprite.bmp\"\n").unwrap();
        fs::write(dir.join("sprite.bmp"), CHESSBOARD_BMP).unwrap();

        let theme = Theme::load(&dir).expect("theme with a real directory must load");
        let bytes = theme.asset_bytes("sprite").expect("declared asset must resolve");
        assert_eq!(bytes, CHESSBOARD_BMP);
        assert!(decode_bmp(&bytes).is_ok());

        assert!(matches!(theme.asset_bytes("missing"), Err(ThemeError::UnknownAsset(_))));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn asset_bytes_without_a_directory_errors() {
        let theme = Theme::default();
        assert!(matches!(theme.asset_bytes("anything"), Err(ThemeError::NoDirectory)));
    }
}
