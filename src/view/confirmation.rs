use crossterm::{
    cursor::MoveTo,
    queue,
    style::{Attribute, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor},
    terminal,
};
use std::io::{self, stdout, Write};

use super::theme::theme;

pub fn render_confirmation(prompt: &str) -> io::Result<()> {
    let t = theme();
    let mut out = stdout();
    let size = terminal::size()?;
    let y = size.1.saturating_sub(3);
    let width = size.0 as usize;

    // Clear the confirmation area
    queue!(out, MoveTo(0, y))?;
    queue!(
        out,
        SetBackgroundColor(t.red),
        SetForegroundColor(t.base),
        SetAttribute(Attribute::Bold)
    )?;
    let line = format!("  {} (y to confirm, any other key to cancel)  ", prompt);
    write!(out, "{:<width$}", line, width = width)?;
    queue!(out, ResetColor, SetAttribute(Attribute::Reset))?;
    out.flush()?;
    Ok(())
}
