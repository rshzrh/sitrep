use crossterm::{
    cursor::MoveTo,
    queue,
    style::{Attribute, Color, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal,
};
use std::io::{self, stdout, Write};

pub fn render_confirmation(prompt: &str) -> io::Result<()> {
    let mut out = stdout();
    let size = terminal::size()?;
    let y = size.1.saturating_sub(3);
    let width = size.0 as usize;

    // Clear the confirmation area
    queue!(out, MoveTo(0, y))?;
    queue!(
        out,
        SetBackgroundColor(Color::DarkRed),
        SetForegroundColor(Color::White),
        SetAttribute(Attribute::Bold)
    )?;
    let line = format!("  {} (y to confirm, any other key to cancel)  ", prompt);
    write!(out, "{:<width$}", line, width = width)?;
    queue!(out, ResetColor, SetAttribute(Attribute::Reset))?;
    out.flush()?;
    Ok(())
}
