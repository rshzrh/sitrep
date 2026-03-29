use crossterm::{
    cursor::MoveTo,
    execute, queue,
    style::{Attribute, ResetColor, SetAttribute, SetForegroundColor},
    terminal,
};
use std::io::{self, stdout, Write};

use super::theme::theme;

pub fn render_splash() -> io::Result<()> {
    let t = theme();
    let mut out = stdout();
    let (cols, rows) = terminal::size()?;

    execute!(out, terminal::Clear(terminal::ClearType::All))?;

    let name = "sitrep";
    let tagline = "Initializing...";

    let center_y = rows / 2;
    let name_x = cols.saturating_sub(name.len() as u16) / 2;
    let tag_x = cols.saturating_sub(tagline.len() as u16) / 2;

    queue!(
        out,
        MoveTo(name_x, center_y),
        SetForegroundColor(t.mauve),
        SetAttribute(Attribute::Bold),
    )?;
    write!(out, "{name}")?;

    queue!(
        out,
        SetAttribute(Attribute::Reset),
        MoveTo(tag_x, center_y + 1),
        SetForegroundColor(t.subtext),
    )?;
    write!(out, "{tagline}")?;

    queue!(out, ResetColor)?;
    out.flush()?;
    Ok(())
}
