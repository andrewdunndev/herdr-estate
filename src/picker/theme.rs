//! herdr's look, copied: the colour tokens the picker paints with and
//! the status glyph set, chosen from herdr's own config file. herdr
//! 0.9.1 gives a plugin no theme: no environment variable, socket method
//! or plugin-context field carries one (#1796 is an open idea; revisit
//! if it ships). So the palettes are copied from herdr's source
//! (app/state.rs:78-150), and the choice between them is read from the
//! file herdr itself reads. Only a fill equal to herdr's resolved
//! `panel_bg` makes the picker and herdr's popup frame one surface.

use std::path::PathBuf;

use ratatui::style::Color;

use crate::picker::core::{Indicators, Status};

/// The herdr colour tokens the picker uses, by herdr's own names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Every cell's background, the same as herdr's popup frame.
    pub panel_bg: Color,
    /// The query and a live row's address.
    pub text: Color,
    /// A dormant or new row's address.
    pub subtext0: Color,
    /// Hints, the count, meta and empty states.
    pub overlay0: Color,
    /// The rule under the filter line.
    pub surface1: Color,
    /// The selection bar; herdr's popup border is drawn in it too.
    pub accent: Color,
    /// Text on the selection bar.
    pub contrast: Color,
    /// Status: working.
    pub yellow: Color,
    /// Status: blocked.
    pub red: Color,
    /// Status: done.
    pub teal: Color,
    /// Status: idle.
    pub green: Color,
}

/// herdr's `contrast()` (overlays.rs:296-301): text on the accent bar
/// is `panel_bg`, or `surface_dim` when `panel_bg` is the terminal's
/// own background and so names no colour.
fn contrast(panel_bg: Color, surface_dim: Color) -> Color {
    match panel_bg {
        Color::Reset => surface_dim,
        c => c,
    }
}

impl Palette {
    /// Catppuccin Mocha, herdr's default theme (app/state.rs:78-100).
    pub fn catppuccin() -> Self {
        let panel_bg = Color::Rgb(24, 24, 37);
        Palette {
            panel_bg,
            text: Color::Rgb(205, 214, 244),
            subtext0: Color::Rgb(166, 173, 200),
            overlay0: Color::Rgb(108, 112, 134),
            surface1: Color::Rgb(69, 71, 90),
            accent: Color::Rgb(137, 180, 250),
            contrast: contrast(panel_bg, Color::Rgb(30, 30, 46)),
            yellow: Color::Rgb(249, 226, 175),
            red: Color::Rgb(243, 139, 168),
            teal: Color::Rgb(148, 226, 213),
            green: Color::Rgb(166, 227, 161),
        }
    }

    /// herdr's 16-colour `terminal` theme (app/state.rs:128-150), which
    /// follows whatever palette the terminal itself has.
    pub fn terminal() -> Self {
        let panel_bg = Color::Reset;
        Palette {
            panel_bg,
            text: Color::Reset,
            subtext0: Color::Gray,
            overlay0: Color::Gray,
            surface1: Color::DarkGray,
            accent: Color::Blue,
            contrast: contrast(panel_bg, Color::DarkGray),
            yellow: Color::Yellow,
            red: Color::LightRed,
            teal: Color::Cyan,
            green: Color::Green,
        }
    }

    /// herdr's `status_color` (client/shell.rs:224-236).
    pub fn status(&self, status: Status) -> Color {
        match status {
            Status::Working => self.yellow,
            Status::Blocked => self.red,
            Status::Done => self.teal,
            Status::Idle => self.green,
            Status::Unknown => self.overlay0,
        }
    }
}

/// A theme name as herdr canonicalizes it (`canonical_theme_name`,
/// config/theme.rs:25-47): lowercased, spaces and underscores as '-',
/// aliases folded. `None` for a name herdr doesn't know, which herdr
/// itself shows as catppuccin.
fn canonical(name: &str) -> Option<&'static str> {
    match name.to_lowercase().replace([' ', '_'], "-").as_str() {
        "catppuccin" | "catppuccin-mocha" => Some("catppuccin"),
        "catppuccin-latte" | "latte" | "light" => Some("catppuccin-latte"),
        "terminal" => Some("terminal"),
        "tokyo-night" | "tokyonight" => Some("tokyo-night"),
        "tokyo-night-day" | "tokyo-day" | "tokyonight-day" => Some("tokyo-night-day"),
        "dracula" => Some("dracula"),
        "nord" => Some("nord"),
        "gruvbox" | "gruvbox-dark" => Some("gruvbox"),
        "gruvbox-light" => Some("gruvbox-light"),
        "one-dark" | "onedark" => Some("one-dark"),
        "one-light" | "onelight" => Some("one-light"),
        "solarized" | "solarized-dark" => Some("solarized"),
        "solarized-light" => Some("solarized-light"),
        "kanagawa" => Some("kanagawa"),
        "kanagawa-lotus" | "lotus" => Some("kanagawa-lotus"),
        "rose-pine" | "rosepine" => Some("rose-pine"),
        "rose-pine-dawn" | "rosepine-dawn" | "dawn" => Some("rose-pine-dawn"),
        "vesper" => Some("vesper"),
        _ => None,
    }
}

/// The palette and glyph set a parsed herdr config gives the picker.
/// Catppuccin when herdr would show catppuccin: no config, no theme
/// name, a name that canonicalizes to catppuccin, or one herdr doesn't
/// know (herdr falls back to catppuccin for those, app/mod.rs:280-287).
/// Anything else herdr would draw differently, and the picker copies
/// only these two palettes, so it takes the terminal one there: any
/// other theme, `auto_switch`, a `[theme.custom*]` override, or the
/// legacy `[ui].accent` override (app/mod.rs:262-270). That is a
/// legible fallback, not a match: under an RGB theme other than
/// catppuccin, herdr's frame and the picker's fill still differ.
pub fn resolve(cfg: Option<&toml::Table>) -> (Palette, Indicators) {
    let section = |name: &str| {
        cfg.and_then(|c| c.get(name))
            .and_then(toml::Value::as_table)
    };
    let theme = section("theme");
    let ui = section("ui");
    let theme_str = |key: &str| theme.and_then(|t| t.get(key)).and_then(toml::Value::as_str);
    let ui_str = |key: &str| ui.and_then(|u| u.get(key)).and_then(toml::Value::as_str);

    let other_theme = theme_str("name")
        .and_then(canonical)
        .is_some_and(|name| name != "catppuccin");
    let auto_switch = theme
        .and_then(|t| t.get("auto_switch"))
        .and_then(toml::Value::as_bool)
        == Some(true);
    let custom = theme.is_some_and(|t| t.keys().any(|k| k.starts_with("custom")));
    let legacy_accent = ui_str("accent").is_some_and(|accent| accent != "cyan");
    let palette = if other_theme || auto_switch || custom || legacy_accent {
        Palette::terminal()
    } else {
        Palette::catppuccin()
    };

    let indicators = match ui_str("status_indicators") {
        Some("symbols") => Indicators::Symbols,
        _ => Indicators::Dots,
    };
    (palette, indicators)
}

/// The config file herdr reads, found the way herdr finds it
/// (config.rs:56; config/io.rs:30-35, 62-68, 195-200):
/// `$HERDR_CONFIG_PATH`, else `$XDG_CONFIG_HOME/herdr/config.toml`, else
/// `$HOME/.config/herdr/config.toml`, else `herdr/config.toml` in the
/// temp dir. A variable counts as set whenever `env::var` reads it, even
/// empty, since that is herdr's `is_ok()` test: an empty
/// `HERDR_CONFIG_PATH` makes herdr read the path "" and fall back to its
/// defaults, not move on to the next place. Everywhere else this crate
/// treats an empty variable as unset (`paths::non_empty_env`), but here
/// the only file worth reading is the one herdr itself reads.
fn config_path() -> PathBuf {
    if let Ok(path) = std::env::var("HERDR_CONFIG_PATH") {
        return PathBuf::from(path);
    }
    let dir = match (std::env::var("XDG_CONFIG_HOME"), std::env::var("HOME")) {
        (Ok(xdg), _) => PathBuf::from(xdg),
        (Err(_), Ok(home)) => PathBuf::from(home).join(".config"),
        (Err(_), Err(_)) => std::env::temp_dir(),
    };
    dir.join("herdr").join("config.toml")
}

/// Read herdr's config and resolve it. A missing or unparseable file
/// gives herdr's own defaults, catppuccin and dots (config/io.rs:172-178),
/// so the picker never fails to open over its look.
pub fn load() -> (Palette, Indicators) {
    let cfg = std::fs::read_to_string(config_path())
        .ok()
        .and_then(|text| text.parse::<toml::Table>().ok());
    resolve(cfg.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{EnvGuard, TempDir};

    fn parse(text: &str) -> toml::Table {
        text.parse().unwrap()
    }

    fn palette_for(text: &str) -> Palette {
        resolve(Some(&parse(text))).0
    }

    #[test]
    fn the_palettes_are_herdrs_tokens() {
        let c = Palette::catppuccin();
        assert_eq!(c.panel_bg, Color::Rgb(0x18, 0x18, 0x25));
        assert_eq!(c.text, Color::Rgb(0xcd, 0xd6, 0xf4));
        assert_eq!(c.subtext0, Color::Rgb(0xa6, 0xad, 0xc8));
        assert_eq!(c.overlay0, Color::Rgb(0x6c, 0x70, 0x86));
        assert_eq!(c.surface1, Color::Rgb(0x45, 0x47, 0x5a));
        assert_eq!(c.accent, Color::Rgb(0x89, 0xb4, 0xfa));
        // panel_bg names a colour, so the bar's text is panel_bg.
        assert_eq!(c.contrast, Color::Rgb(0x18, 0x18, 0x25));
        assert_eq!(c.yellow, Color::Rgb(0xf9, 0xe2, 0xaf));
        assert_eq!(c.red, Color::Rgb(0xf3, 0x8b, 0xa8));
        assert_eq!(c.teal, Color::Rgb(0x94, 0xe2, 0xd5));
        assert_eq!(c.green, Color::Rgb(0xa6, 0xe3, 0xa1));

        let t = Palette::terminal();
        assert_eq!(t.panel_bg, Color::Reset);
        assert_eq!(t.text, Color::Reset);
        assert_eq!(t.subtext0, Color::Gray);
        assert_eq!(t.overlay0, Color::Gray);
        assert_eq!(t.surface1, Color::DarkGray);
        assert_eq!(t.accent, Color::Blue);
        // panel_bg is Reset, so the bar's text falls to surface_dim.
        assert_eq!(t.contrast, Color::DarkGray);
        assert_eq!(t.yellow, Color::Yellow);
        assert_eq!(t.red, Color::LightRed);
        assert_eq!(t.teal, Color::Cyan);
        assert_eq!(t.green, Color::Green);
    }

    #[test]
    fn each_status_takes_herdrs_status_colour() {
        let p = Palette::catppuccin();
        assert_eq!(p.status(Status::Working), p.yellow);
        assert_eq!(p.status(Status::Blocked), p.red);
        assert_eq!(p.status(Status::Done), p.teal);
        assert_eq!(p.status(Status::Idle), p.green);
        assert_eq!(p.status(Status::Unknown), p.overlay0);
    }

    #[test]
    fn theme_names_canonicalize_as_herdrs_do() {
        let table = [
            ("catppuccin", "catppuccin"),
            ("Catppuccin Mocha", "catppuccin"),
            ("catppuccin_latte", "catppuccin-latte"),
            ("latte", "catppuccin-latte"),
            ("light", "catppuccin-latte"),
            ("Terminal", "terminal"),
            ("tokyo night", "tokyo-night"),
            ("tokyonight", "tokyo-night"),
            ("tokyo-night-day", "tokyo-night-day"),
            ("tokyo-day", "tokyo-night-day"),
            ("tokyonight-day", "tokyo-night-day"),
            ("dracula", "dracula"),
            ("nord", "nord"),
            ("gruvbox", "gruvbox"),
            ("gruvbox-dark", "gruvbox"),
            ("gruvbox-light", "gruvbox-light"),
            ("one-dark", "one-dark"),
            ("onedark", "one-dark"),
            ("one-light", "one-light"),
            ("onelight", "one-light"),
            ("solarized", "solarized"),
            ("solarized-dark", "solarized"),
            ("solarized-light", "solarized-light"),
            ("kanagawa", "kanagawa"),
            ("kanagawa-lotus", "kanagawa-lotus"),
            ("lotus", "kanagawa-lotus"),
            ("rose-pine", "rose-pine"),
            ("rosepine", "rose-pine"),
            ("rose-pine-dawn", "rose-pine-dawn"),
            ("rosepine-dawn", "rose-pine-dawn"),
            ("dawn", "rose-pine-dawn"),
            ("vesper", "vesper"),
        ];
        for (name, want) in table {
            assert_eq!(canonical(name), Some(want), "{name:?}");
        }
        for name in ["nrod", "", "catppuccin mocha latte"] {
            assert_eq!(canonical(name), None, "{name:?}");
        }
    }

    #[test]
    fn resolve_picks_catppuccin_wherever_herdr_shows_it() {
        assert_eq!(
            resolve(None),
            (Palette::catppuccin(), Indicators::Dots),
            "no config at all"
        );
        for text in [
            "",
            "[theme]\n",
            "[theme]\nname = \"Catppuccin Mocha\"\n",
            // herdr falls back to catppuccin for a name it doesn't know.
            "[theme]\nname = \"nrod\"\n",
            "[theme]\nauto_switch = false\n",
            // cyan is the legacy accent's default: no override.
            "[ui]\naccent = \"cyan\"\n",
        ] {
            assert_eq!(palette_for(text), Palette::catppuccin(), "{text:?}");
        }
    }

    #[test]
    fn resolve_falls_back_to_the_terminal_palette_for_any_other_look() {
        for text in [
            "[theme]\nname = \"Terminal\"\n",
            "[theme]\nname = \"nord\"\n",
            "[theme]\nauto_switch = true\n",
            "[theme.custom]\naccent = \"#f5c2e7\"\n",
            "[theme.custom.dark]\naccent = \"#f5c2e7\"\n",
            "[ui]\naccent = \"magenta\"\n",
        ] {
            assert_eq!(palette_for(text), Palette::terminal(), "{text:?}");
        }
    }

    #[test]
    fn resolve_reads_the_status_indicators() {
        let ind = |text: &str| resolve(Some(&parse(text))).1;
        assert_eq!(
            ind("[ui]\nstatus_indicators = \"symbols\"\n"),
            Indicators::Symbols
        );
        assert_eq!(
            ind("[ui]\nstatus_indicators = \"dots\"\n"),
            Indicators::Dots
        );
        assert_eq!(ind("[ui]\n"), Indicators::Dots);
        // The operator's own config: no [theme], symbols.
        assert_eq!(
            resolve(Some(&parse("[ui]\nstatus_indicators = \"symbols\"\n"))),
            (Palette::catppuccin(), Indicators::Symbols)
        );
    }

    #[test]
    fn the_config_path_prefers_herdr_config_path() {
        let _g = EnvGuard::set(&[
            ("HERDR_CONFIG_PATH", "/cfg/herdr.toml"),
            ("XDG_CONFIG_HOME", "/xdg"),
            ("HOME", "/home/op"),
        ]);
        assert_eq!(config_path(), PathBuf::from("/cfg/herdr.toml"));
    }

    #[test]
    fn the_config_path_falls_back_to_xdg_config_home() {
        let _g = EnvGuard::new(
            &[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/home/op")],
            &["HERDR_CONFIG_PATH"],
        );
        assert_eq!(config_path(), PathBuf::from("/xdg/herdr/config.toml"));
    }

    #[test]
    fn the_config_path_falls_back_to_home() {
        let _g = EnvGuard::new(
            &[("HOME", "/home/op")],
            &["HERDR_CONFIG_PATH", "XDG_CONFIG_HOME"],
        );
        assert_eq!(
            config_path(),
            PathBuf::from("/home/op/.config/herdr/config.toml")
        );
    }

    #[test]
    fn with_no_home_the_config_path_is_herdrs_temp_dir_fallback() {
        let _g = EnvGuard::unset(&["HERDR_CONFIG_PATH", "XDG_CONFIG_HOME", "HOME"]);
        assert_eq!(
            config_path(),
            std::env::temp_dir().join("herdr").join("config.toml")
        );
    }

    #[test]
    fn an_empty_herdr_config_path_is_set_as_herdr_reads_it() {
        // herdr reads the path "" (which fails, so its defaults), never
        // the XDG file behind it, so the picker must not pick that
        // file's symbols up either.
        let d = TempDir::new();
        std::fs::create_dir_all(d.path().join("herdr")).unwrap();
        std::fs::write(
            d.path().join("herdr").join("config.toml"),
            "[ui]\nstatus_indicators = \"symbols\"\n",
        )
        .unwrap();
        let _g = EnvGuard::set(&[
            ("HERDR_CONFIG_PATH", ""),
            ("XDG_CONFIG_HOME", d.path().to_str().unwrap()),
        ]);
        assert_eq!(config_path(), PathBuf::from(""));
        assert_eq!(load(), (Palette::catppuccin(), Indicators::Dots));
    }

    #[test]
    fn an_empty_xdg_config_home_is_set_as_herdr_reads_it() {
        // herdr joins "" and reads the relative herdr/config.toml, never
        // $HOME's file.
        let d = TempDir::new();
        let home_cfg = d.path().join(".config").join("herdr");
        std::fs::create_dir_all(&home_cfg).unwrap();
        std::fs::write(
            home_cfg.join("config.toml"),
            "[ui]\nstatus_indicators = \"symbols\"\n",
        )
        .unwrap();
        let _g = EnvGuard::new(
            &[
                ("XDG_CONFIG_HOME", ""),
                ("HOME", d.path().to_str().unwrap()),
            ],
            &["HERDR_CONFIG_PATH"],
        );
        assert_eq!(config_path(), PathBuf::from("herdr").join("config.toml"));
    }

    #[test]
    fn load_reads_the_file_the_path_names() {
        let d = TempDir::new();
        let path = d.path().join("config.toml");
        std::fs::write(
            &path,
            "[theme]\nname = \"nord\"\n[ui]\nstatus_indicators = \"symbols\"\n",
        )
        .unwrap();
        let _g = EnvGuard::set(&[("HERDR_CONFIG_PATH", path.to_str().unwrap())]);
        assert_eq!(load(), (Palette::terminal(), Indicators::Symbols));
    }

    #[test]
    fn a_missing_or_unparseable_config_gives_herdrs_defaults() {
        let d = TempDir::new();
        let broken = d.path().join("broken.toml");
        std::fs::write(&broken, "[theme\nname = \"nord\"\n").unwrap();
        let missing = d.path().join("missing.toml");
        for path in [&broken, &missing] {
            let _g = EnvGuard::set(&[("HERDR_CONFIG_PATH", path.to_str().unwrap())]);
            assert_eq!(
                load(),
                (Palette::catppuccin(), Indicators::Dots),
                "{path:?}"
            );
        }
    }
}
