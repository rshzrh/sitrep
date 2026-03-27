use crossterm::style::Color;

/// Catppuccin Mocha color theme for the TUI.
pub struct Theme {
    pub base: Color,
    pub text: Color,
    pub subtext: Color,
    pub surface: Color,
    pub overlay: Color,

    pub mauve: Color,
    pub lavender: Color,
    pub teal: Color,
    pub green: Color,
    pub yellow: Color,
    pub peach: Color,
    pub red: Color,
    pub flamingo: Color,
    pub sky: Color,
    pub sapphire: Color,

    // Semantic aliases
    pub bar_filled: Color,
    pub bar_empty: Color,
    pub selected_bg: Color,
    pub selected_fg: Color,
    pub header_fg: Color,
    pub tab_active_bg: Color,
    pub tab_active_fg: Color,
    pub tab_inactive_fg: Color,
    pub help_key: Color,
    pub help_desc: Color,
    pub separator: Color,
}

impl Theme {
    pub fn catppuccin_mocha() -> Self {
        let base = Color::Rgb { r: 30, g: 30, b: 46 };
        let text = Color::Rgb { r: 205, g: 214, b: 244 };
        let subtext = Color::Rgb { r: 166, g: 173, b: 200 };
        let surface = Color::Rgb { r: 49, g: 50, b: 68 };
        let overlay = Color::Rgb { r: 69, g: 71, b: 90 };

        let mauve = Color::Rgb { r: 203, g: 166, b: 247 };
        let lavender = Color::Rgb { r: 180, g: 190, b: 254 };
        let teal = Color::Rgb { r: 148, g: 226, b: 213 };
        let green = Color::Rgb { r: 166, g: 227, b: 161 };
        let yellow = Color::Rgb { r: 249, g: 226, b: 175 };
        let peach = Color::Rgb { r: 250, g: 179, b: 135 };
        let red = Color::Rgb { r: 243, g: 139, b: 168 };
        let flamingo = Color::Rgb { r: 242, g: 205, b: 205 };
        let sky = Color::Rgb { r: 137, g: 220, b: 235 };
        let sapphire = Color::Rgb { r: 116, g: 199, b: 236 };

        Self {
            base,
            text,
            subtext,
            surface,
            overlay,
            mauve,
            lavender,
            teal,
            green,
            yellow,
            peach,
            red,
            flamingo,
            sky,
            sapphire,
            bar_filled: teal,
            bar_empty: surface,
            selected_bg: overlay,
            selected_fg: text,
            header_fg: lavender,
            tab_active_bg: mauve,
            tab_active_fg: base,
            tab_inactive_fg: subtext,
            help_key: mauve,
            help_desc: subtext,
            separator: surface,
        }
    }
}

/// Global theme accessor. Returns the Catppuccin Mocha theme.
pub fn theme() -> Theme {
    Theme::catppuccin_mocha()
}
