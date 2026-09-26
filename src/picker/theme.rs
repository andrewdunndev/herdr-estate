// Portions adapted from herdr v0.9.1 (github.com/herdrdev/herdr, tag
// 8544776, Apache-2.0, see /LICENSE-APACHE-HERDR; Catppuccin palettes
// MIT): palettes from src/app/state.rs:78-525; canonical() and
// parse_color() from src/config/theme.rs:25-47,155-219 (changed: .get()
// slicing); resolution from src/app/mod.rs:219-332.

use std::path::PathBuf;

use ratatui::style::Color;

use crate::picker::core::{Indicators, Status};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub panel_bg: Color,
    pub text: Color,
    pub subtext0: Color,
    pub overlay0: Color,
    pub surface1: Color,
    pub accent: Color,
    pub surface_dim: Color,
    pub yellow: Color,
    pub red: Color,
    pub teal: Color,
    pub green: Color,
}

#[rustfmt::skip]
const THEMES: [(&str, [u32; 11]); 17] = [
    ("catppuccin", [0x181825, 0xcdd6f4, 0xa6adc8, 0x6c7086, 0x45475a, 0x89b4fa, 0x1e1e2e, 0xf9e2af, 0xf38ba8, 0x94e2d5, 0xa6e3a1]),
    ("catppuccin-latte", [0xeff1f5, 0x4c4f69, 0x6c6f85, 0x9ca0b0, 0xbcc0cc, 0x1e66f5, 0xe6e9ef, 0xdf8e1d, 0xd20f39, 0x179299, 0x40a02b]),
    ("tokyo-night", [0x1a1b26, 0xc0caf5, 0xa9b1d6, 0x565f89, 0x414868, 0x7aa2f7, 0x1a1b26, 0xe0af68, 0xf7768e, 0x7dcfff, 0x9ece6a]),
    ("tokyo-night-day", [0xe1e2e7, 0x3760bf, 0x6172b0, 0x8990b3, 0xa8aecb, 0x2e7de9, 0xd2d3da, 0x8c6c3e, 0xf52a65, 0x118c74, 0x587539]),
    ("dracula", [0x282a36, 0xf8f8f2, 0xd2d2dc, 0x6272a4, 0x6272a4, 0xbd93f9, 0x282a36, 0xf1fa8c, 0xff5555, 0x8be9fd, 0x50fa7b]),
    ("nord", [0x2e3440, 0xeceff4, 0xd8dee9, 0x4c566a, 0x434c5e, 0x88c0d0, 0x2e3440, 0xebcb8b, 0xbf616a, 0x8fbcbb, 0xa3be8c]),
    ("gruvbox", [0x282828, 0xebdbb2, 0xd5c4a1, 0x928374, 0x504945, 0xd79921, 0x282828, 0xfabd2f, 0xfb4934, 0x8ec07c, 0xb8bb26]),
    ("gruvbox-light", [0xfbf1c7, 0x3c3836, 0x504945, 0x928374, 0xd5c4a1, 0x076678, 0xf2e5bc, 0xb57614, 0x9d0006, 0x427b58, 0x79740e]),
    ("one-dark", [0x282c34, 0xabb2bf, 0x969ca8, 0x5c6370, 0x3e4451, 0x61afef, 0x282c34, 0xe5c07b, 0xe06c75, 0x56b6c2, 0x98c379]),
    ("one-light", [0xfafafa, 0x383a42, 0x686b77, 0xa0a1a7, 0xe5e5e6, 0x4078f2, 0xf5f5f6, 0xc18401, 0xe45649, 0x0184bc, 0x50a14f]),
    ("solarized", [0x002b36, 0x93a1a1, 0x839496, 0x586e75, 0x586e75, 0x268bd2, 0x002b36, 0xb58900, 0xdc322f, 0x2aa198, 0x859900]),
    ("solarized-light", [0xfdf6e3, 0x657b83, 0x839496, 0x93a1a1, 0x93a1a1, 0x268bd2, 0xeee8d5, 0xb58900, 0xdc322f, 0x2aa198, 0x859900]),
    ("kanagawa", [0x1f1f28, 0xdcd7ba, 0xc8c3aa, 0x727169, 0x363646, 0x7e9cd8, 0x1f1f28, 0xc0a36e, 0xc34043, 0x7fb4ca, 0x76946a]),
    ("kanagawa-lotus", [0xf2ecbc, 0x545464, 0x43436c, 0xa09cac, 0xc9cbd1, 0x4d699b, 0xd5cea3, 0x77713f, 0xc84053, 0x4e8ca2, 0x6f894e]),
    ("rose-pine", [0x191724, 0xe0def4, 0xc8c5dc, 0x6e6a86, 0x26233a, 0xc4a7e7, 0x26233a, 0xf6c177, 0xeb6f92, 0x9ccfd8, 0x31748f]),
    ("rose-pine-dawn", [0xfaf4ed, 0x464261, 0x797593, 0x9893a5, 0xfffaf3, 0x907aa9, 0xf2e9e1, 0xea9d34, 0xb4637a, 0x56949f, 0x286983]),
    ("vesper", [0x1a1a1a, 0xffffff, 0xa0a0a0, 0x5c5c5c, 0x282828, 0xffc799, 0x101010, 0xffc799, 0xff8080, 0x66ddcc, 0x99ffe4]),
];

const fn rgb(x: u32) -> Color {
    Color::Rgb((x >> 16) as u8, (x >> 8) as u8, x as u8)
}

fn from_row(row: [u32; 11]) -> Palette {
    let [panel_bg, text, subtext0, overlay0, surface1, accent, surface_dim, yellow, red, teal, green] =
        row.map(rgb);
    Palette {
        panel_bg,
        text,
        subtext0,
        overlay0,
        surface1,
        accent,
        surface_dim,
        yellow,
        red,
        teal,
        green,
    }
}

impl Palette {
    pub fn catppuccin() -> Self {
        from_row(THEMES[0].1)
    }

    pub fn terminal() -> Self {
        Palette {
            panel_bg: Color::Reset,
            text: Color::Reset,
            subtext0: Color::Gray,
            overlay0: Color::Gray,
            surface1: Color::DarkGray,
            accent: Color::Blue,
            surface_dim: Color::DarkGray,
            yellow: Color::Yellow,
            red: Color::LightRed,
            teal: Color::Cyan,
            green: Color::Green,
        }
    }

    pub fn contrast(&self) -> Color {
        match self.panel_bg {
            Color::Reset => self.surface_dim,
            c => c,
        }
    }

    pub fn status(&self, status: Status) -> Color {
        match status {
            Status::Working => self.yellow,
            Status::Blocked => self.red,
            Status::Done => self.teal,
            Status::Idle => self.green,
            Status::Unknown => self.overlay0,
        }
    }

    fn apply(&mut self, t: &toml::Table) {
        let tokens = [
            ("panel_bg", &mut self.panel_bg),
            ("text", &mut self.text),
            ("subtext0", &mut self.subtext0),
            ("overlay0", &mut self.overlay0),
            ("surface1", &mut self.surface1),
            ("accent", &mut self.accent),
            ("surface_dim", &mut self.surface_dim),
            ("yellow", &mut self.yellow),
            ("red", &mut self.red),
            ("teal", &mut self.teal),
            ("green", &mut self.green),
        ];
        for (key, token) in tokens {
            if let Some(value) = t.get(key).and_then(toml::Value::as_str) {
                *token = parse_color(value);
            }
        }
    }
}

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

fn builtin(name: &str) -> Option<Palette> {
    match canonical(name)? {
        "terminal" => Some(Palette::terminal()),
        c => THEMES
            .iter()
            .find(|(n, _)| *n == c)
            .map(|(_, row)| from_row(*row)),
    }
}

fn parse_color(s: &str) -> Color {
    let s = s.trim().to_lowercase();

    if matches!(s.as_str(), "reset" | "default" | "none" | "transparent") {
        return Color::Reset;
    }

    if let Some(hex) = s.strip_prefix('#') {
        if hex.len() == 6 {
            let byte = |i: usize| {
                hex.get(i..i + 2)
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
            };
            if let (Some(r), Some(g), Some(b)) = (byte(0), byte(2), byte(4)) {
                return Color::Rgb(r, g, b);
            }
        } else if hex.len() == 3 {
            let chars: Vec<u8> = hex
                .chars()
                .filter_map(|c| u8::from_str_radix(&c.to_string(), 16).ok())
                .collect();
            if chars.len() == 3 {
                return Color::Rgb(chars[0] * 17, chars[1] * 17, chars[2] * 17);
            }
        }
    }

    if let Some(inner) = s.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')) {
        let parts: Vec<&str> = inner.split(',').collect();
        if parts.len() == 3 {
            if let (Ok(r), Ok(g), Ok(b)) = (
                parts[0].trim().parse::<u8>(),
                parts[1].trim().parse::<u8>(),
                parts[2].trim().parse::<u8>(),
            ) {
                return Color::Rgb(r, g, b);
            }
        }
    }

    match s.as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" | "purple" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::White,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        _ => Color::Cyan,
    }
}

const SIBLINGS: [(&str, &str); 7] = [
    ("catppuccin", "catppuccin-latte"),
    ("tokyo-night", "tokyo-night-day"),
    ("gruvbox", "gruvbox-light"),
    ("one-dark", "one-light"),
    ("solarized", "solarized-light"),
    ("kanagawa", "kanagawa-lotus"),
    ("rose-pine", "rose-pine-dawn"),
];

fn dark_partner(name: &str) -> &str {
    let c = canonical(name);
    SIBLINGS
        .iter()
        .find(|(_, light)| c == Some(*light))
        .map_or(name, |(dark, _)| dark)
}

pub fn resolve(cfg: Option<&toml::Table>) -> (Palette, Indicators) {
    fn get<'a>(t: Option<&'a toml::Table>, key: &str) -> Option<&'a toml::Value> {
        t?.get(key)
    }
    let theme = get(cfg, "theme").and_then(toml::Value::as_table);
    let ui = get(cfg, "ui").and_then(toml::Value::as_table);
    let theme_str = |key: &str| get(theme, key).and_then(toml::Value::as_str);
    let ui_str = |key: &str| get(ui, key).and_then(toml::Value::as_str);
    let custom = get(theme, "custom").and_then(toml::Value::as_table);

    let manual = theme_str("name").unwrap_or("catppuccin");
    let (name, mode) = if get(theme, "auto_switch").and_then(toml::Value::as_bool) == Some(true) {
        (
            theme_str("dark_name").unwrap_or_else(|| dark_partner(manual)),
            get(custom, "dark").and_then(toml::Value::as_table),
        )
    } else {
        (manual, None)
    };

    let mut palette = builtin(name).unwrap_or_else(Palette::catppuccin);
    if let Some(custom) = custom {
        palette.apply(custom);
    }
    let custom_accent = get(custom, "accent").and_then(toml::Value::as_str);
    if let Some(accent) = ui_str("accent").filter(|a| *a != "cyan" && custom_accent.is_none()) {
        palette.accent = parse_color(accent);
    }
    if let Some(mode) = mode {
        palette.apply(mode);
    }

    let indicators = match ui_str("status_indicators") {
        Some("symbols") => Indicators::Symbols,
        _ => Indicators::Dots,
    };
    (palette, indicators)
}

fn config_path() -> PathBuf {
    // herdr treats an empty HERDR_CONFIG_PATH as set, so this does too.
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
        assert_eq!(c.surface_dim, Color::Rgb(0x1e, 0x1e, 0x2e));
        assert_eq!(c.contrast(), Color::Rgb(0x18, 0x18, 0x25));
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
        assert_eq!(t.surface_dim, Color::DarkGray);
        assert_eq!(t.contrast(), Color::DarkGray);
        assert_eq!(t.yellow, Color::Yellow);
        assert_eq!(t.red, Color::LightRed);
        assert_eq!(t.teal, Color::Cyan);
        assert_eq!(t.green, Color::Green);
    }

    #[test]
    fn vesper_and_latte_are_herdrs_tokens() {
        let vesper = builtin("vesper").unwrap();
        assert_eq!(
            vesper,
            Palette {
                panel_bg: Color::Rgb(26, 26, 26),
                text: Color::Rgb(255, 255, 255),
                subtext0: Color::Rgb(160, 160, 160),
                overlay0: Color::Rgb(92, 92, 92),
                surface1: Color::Rgb(40, 40, 40),
                accent: Color::Rgb(255, 199, 153),
                surface_dim: Color::Rgb(16, 16, 16),
                yellow: Color::Rgb(255, 199, 153),
                red: Color::Rgb(255, 128, 128),
                teal: Color::Rgb(102, 221, 204),
                green: Color::Rgb(153, 255, 228),
            }
        );
        assert_eq!(vesper.contrast(), Color::Rgb(26, 26, 26));
        let latte = builtin("catppuccin-latte").unwrap();
        assert_eq!(
            latte,
            Palette {
                panel_bg: Color::Rgb(239, 241, 245),
                text: Color::Rgb(76, 79, 105),
                subtext0: Color::Rgb(108, 111, 133),
                overlay0: Color::Rgb(156, 160, 176),
                surface1: Color::Rgb(188, 192, 204),
                accent: Color::Rgb(30, 102, 245),
                surface_dim: Color::Rgb(230, 233, 239),
                yellow: Color::Rgb(223, 142, 29),
                red: Color::Rgb(210, 15, 57),
                teal: Color::Rgb(23, 146, 153),
                green: Color::Rgb(64, 160, 43),
            }
        );
        assert_eq!(latte.contrast(), Color::Rgb(239, 241, 245));
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
            assert!(builtin(name).is_some(), "{name:?}");
        }
        for name in ["nrod", "", "catppuccin mocha latte"] {
            assert_eq!(canonical(name), None, "{name:?}");
            assert_eq!(builtin(name), None, "{name:?}");
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
            "[theme]\nname = \"nrod\"\n",
            "[theme]\nauto_switch = false\n",
            "[ui]\naccent = \"cyan\"\n",
            "[theme.custom]\n",
            "[theme.custom_x]\naccent = \"#f5c2e7\"\n",
            "[theme.custom]\nblue = \"#f5c2e7\"\nacent = \"#f5c2e7\"\ntext = 5\n",
            "[theme.custom.dark]\naccent = \"#f5c2e7\"\n",
        ] {
            assert_eq!(palette_for(text), Palette::catppuccin(), "{text:?}");
        }
    }

    #[test]
    fn parse_color_reads_what_herdr_reads_quirks_included() {
        assert_eq!(parse_color("#FFF"), Color::Rgb(255, 255, 255));
        assert_eq!(parse_color("#0a0B0c"), Color::Rgb(10, 11, 12));
        assert_eq!(parse_color("rgb(1, 2, 3)"), Color::Rgb(1, 2, 3));
        for reset in [" Transparent ", "reset", "default", "none"] {
            assert_eq!(parse_color(reset), Color::Reset, "{reset:?}");
        }
        for (name, want) in [
            ("black", Color::Black),
            ("red", Color::Red),
            ("green", Color::Green),
            ("yellow", Color::Yellow),
            ("blue", Color::Blue),
            ("magenta", Color::Magenta),
            ("purple", Color::Magenta),
            ("cyan", Color::Cyan),
            ("white", Color::White),
            ("gray", Color::Gray),
            ("grey", Color::Gray),
            ("darkgray", Color::DarkGray),
            ("darkgrey", Color::DarkGray),
            ("lightred", Color::LightRed),
            ("lightgreen", Color::LightGreen),
            ("lightyellow", Color::LightYellow),
            ("lightblue", Color::LightBlue),
            ("lightmagenta", Color::LightMagenta),
            ("lightcyan", Color::LightCyan),
        ] {
            assert_eq!(parse_color(name), want, "{name:?}");
        }
        assert_eq!(parse_color("#+f+f+f"), Color::Rgb(15, 15, 15));
        for bad in [
            "color5",
            "5",
            "#ffg",
            "#fffffg",
            "rgb(1, 2)",
            "rgb(1, 2, 256)",
            "#aéaé",
        ] {
            assert_eq!(parse_color(bad), Color::Cyan, "{bad:?}");
        }
    }

    #[test]
    fn theme_custom_overrides_the_base_theme_token_by_token() {
        let mut want = builtin("nord").unwrap();
        want.accent = Color::Rgb(0xf5, 0xc2, 0xe7);
        assert_eq!(
            palette_for("[theme]\nname = \"nord\"\n[theme.custom]\naccent = \"#f5c2e7\"\n"),
            want
        );
        let keys =
            "panel_bg text subtext0 overlay0 surface1 accent surface_dim yellow red teal green";
        let mut text = String::from("[theme]\nname = \"nord\"\n[theme.custom]\n");
        for (i, key) in keys.split(' ').enumerate() {
            text += &format!("{key} = \"#0000{:02x}\"\n", i + 1);
        }
        assert_eq!(
            palette_for(&text),
            from_row([1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11])
        );
    }

    #[test]
    fn a_transparent_custom_panel_puts_surface_dim_under_the_bar_text() {
        let p =
            palette_for("[theme]\nname = \"vesper\"\n[theme.custom]\npanel_bg = \"transparent\"\n");
        assert_eq!(p.panel_bg, Color::Reset);
        assert_eq!(p.contrast(), Color::Rgb(0x10, 0x10, 0x10));
    }

    #[test]
    fn the_legacy_ui_accent_applies_where_herdr_applies_it() {
        let accent = |text: &str| palette_for(text).accent;
        assert_eq!(accent("[ui]\naccent = \"magenta\"\n"), Color::Magenta);
        assert_eq!(accent("[ui]\naccent = \"Cyan\"\n"), Color::Cyan);
        assert_eq!(
            accent("[theme.custom]\naccent = \"#010203\"\n[ui]\naccent = \"magenta\"\n"),
            Color::Rgb(1, 2, 3)
        );
    }

    #[test]
    fn auto_switch_shows_the_theme_herdr_shows_on_an_appearance_it_cant_tell() {
        let auto = |rest: &str| palette_for(&format!("[theme]\nauto_switch = true\n{rest}"));
        let theme = |name: &str| builtin(name).unwrap();
        assert_eq!(
            palette_for("[theme]\nname = \"tokyo-night-day\"\n"),
            theme("tokyo-night-day")
        );
        assert_eq!(auto("name = \"tokyo-night-day\"\n"), theme("tokyo-night"));
        assert_eq!(
            auto("name = \"tokyo-night-day\"\ndark_name = \"nord\"\n"),
            theme("nord")
        );
        assert_eq!(
            auto("name = \"tokyo-night-day\"\ndark_name = \"nrod\"\n"),
            Palette::catppuccin()
        );
        assert_eq!(auto("name = \"vesper\"\n"), theme("vesper"));
    }

    #[test]
    fn under_auto_switch_theme_custom_dark_lands_last() {
        let p = palette_for(
            "[theme]\nauto_switch = true\n\
             [theme.custom]\ntext = \"#010101\"\naccent = \"#020202\"\n\
             [theme.custom.dark]\naccent = \"#030303\"\n",
        );
        assert_eq!(p.text, Color::Rgb(1, 1, 1));
        assert_eq!(p.accent, Color::Rgb(3, 3, 3));
        let p = palette_for(
            "[theme]\nauto_switch = true\n\
             [theme.custom.dark]\naccent = \"#030303\"\n\
             [ui]\naccent = \"magenta\"\n",
        );
        assert_eq!(p.accent, Color::Rgb(3, 3, 3));
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
        assert_eq!(load(), (builtin("nord").unwrap(), Indicators::Symbols));
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
