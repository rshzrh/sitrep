use crossterm::{
    cursor, queue,
    style::{Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor},
};
use std::io::{self, Write};

use super::theme::theme;

/// Truncate a string to at most `max_len` characters (not bytes), appending "..."
/// if truncated. Safe for multi-byte UTF-8.
pub fn truncate_str(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        s.to_string()
    } else if max_len <= 3 {
        s.chars().take(max_len).collect()
    } else {
        let keep = max_len - 3;
        let truncated: String = s.chars().take(keep).collect();
        format!("{}...", truncated)
    }
}

/// Truncate a string to at most `max_len` characters for display. Returns a &str
/// slice up to the last valid char boundary within `max_len` bytes.
pub fn safe_truncate(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        return s;
    }
    let mut end = max_len;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

pub fn writeln(out: &mut impl Write, text: &str) -> io::Result<()> {
    write!(out, "{}\r\n", text)
}

#[allow(dead_code)]
pub fn write_section_header(out: &mut impl Write, text: &str, selected: bool) -> io::Result<()> {
    let t = theme();
    if selected {
        queue!(
            io::stdout(),
            SetBackgroundColor(t.selected_bg),
            SetForegroundColor(t.header_fg)
        )?;
    } else {
        queue!(
            io::stdout(),
            SetForegroundColor(t.header_fg),
            SetAttribute(Attribute::Bold)
        )?;
    }
    write!(out, "{}\r\n", text)?;
    queue!(io::stdout(), ResetColor, SetAttribute(Attribute::Reset))?;
    Ok(())
}

pub fn write_selectable(out: &mut impl Write, text: &str, selected: bool) -> io::Result<()> {
    let t = theme();
    if selected {
        queue!(
            io::stdout(),
            SetBackgroundColor(t.selected_bg),
            SetForegroundColor(t.selected_fg)
        )?;
    }
    write!(out, "{}\r\n", text)?;
    if selected {
        queue!(io::stdout(), ResetColor)?;
    }
    Ok(())
}

pub fn format_bytes_rate(bytes: u64) -> String {
    if bytes > 1_048_576 {
        format!("{:.2} MB/s", bytes as f64 / 1_048_576.0)
    } else if bytes > 1024 {
        format!("{:.2} KB/s", bytes as f64 / 1024.0)
    } else {
        format!("{} B/s", bytes)
    }
}

#[allow(dead_code)]
pub fn progress_bar(percent: f64, width: usize) -> String {
    let filled = ((percent / 100.0) * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("[{}{}]", "\u{2588}".repeat(filled), "\u{2591}".repeat(empty))
}

#[allow(dead_code)]
pub fn format_number(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut result = String::new();
    for (i, &b) in bytes.iter().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(b as char);
    }
    result.chars().rev().collect()
}

/// Render a colored htop-style bar to stdout.
///
/// Output looks like: ` CPU [||||||||      ] 65.2%  13.5G/16.0G`
///
/// - Label in theme.text (3 chars, right-aligned)
/// - Brackets in theme.subtext
/// - Filled portion (`|` chars) in theme.bar_filled (teal)
/// - Empty portion (spaces) with theme.bar_empty background
/// - Percent in theme.text
/// - Detail in theme.subtext
pub fn render_bar(
    out: &mut impl Write,
    label: &str,
    percent: f64,
    detail: &str,
    bar_width: usize,
) -> io::Result<()> {
    let t = theme();
    let clamped = percent.clamp(0.0, 100.0);
    let filled = ((clamped / 100.0) * bar_width as f64).round() as usize;
    let empty = bar_width.saturating_sub(filled);

    // Label (3 chars, right-aligned)
    queue!(io::stdout(), SetForegroundColor(t.text))?;
    write!(out, " {:>3} ", label)?;

    // Opening bracket
    queue!(io::stdout(), SetForegroundColor(t.subtext))?;
    write!(out, "[")?;

    // Filled portion: teal '|' chars
    queue!(io::stdout(), SetForegroundColor(t.bar_filled))?;
    for _ in 0..filled {
        write!(out, "|")?;
    }

    // Empty portion: spaces with surface background
    queue!(
        io::stdout(),
        SetBackgroundColor(t.bar_empty)
    )?;
    for _ in 0..empty {
        write!(out, " ")?;
    }
    queue!(io::stdout(), ResetColor)?;

    // Closing bracket
    queue!(io::stdout(), SetForegroundColor(t.subtext))?;
    write!(out, "]")?;

    // Percent
    queue!(io::stdout(), SetForegroundColor(t.text))?;
    write!(out, " {:>5.1}%", clamped)?;

    // Detail
    if !detail.is_empty() {
        queue!(io::stdout(), SetForegroundColor(t.subtext))?;
        write!(out, "  {}", detail)?;
    }

    queue!(io::stdout(), ResetColor)?;
    Ok(())
}

/// Render a themed help footer at position (0, y).
///
/// Each item: key in mauve, `:` in subtext, description in subtext, double-space between items.
pub fn render_help_footer(
    out: &mut impl Write,
    items: &[(&str, &str)],
    width: usize,
    y: u16,
) -> io::Result<()> {
    let t = theme();
    queue!(io::stdout(), cursor::MoveTo(1, y))?;

    let mut col: usize = 1;
    for (i, (key, desc)) in items.iter().enumerate() {
        if i > 0 {
            queue!(out, Print("  "))?;
            col += 2;
        }
        // key in mauve
        queue!(
            out,
            SetForegroundColor(t.help_key),
            Print(key.to_string()),
        )?;
        col += key.chars().count();

        // colon + description in subtext
        queue!(
            out,
            SetForegroundColor(t.help_desc),
            Print(format!(":{}", desc)),
        )?;
        col += 1 + desc.chars().count();
    }

    // Clear remaining width
    if col < width {
        queue!(out, Print(" ".repeat(width - col)))?;
    }

    queue!(out, ResetColor)?;
    Ok(())
}

/// Format bytes into human-readable form (e.g., "973M", "1.5G", "256K").
pub fn format_mem_human(bytes: u64) -> String {
    let gb = bytes as f64 / 1_073_741_824.0;
    let mb = bytes as f64 / 1_048_576.0;
    let kb = bytes as f64 / 1024.0;
    if gb >= 1.0 {
        format!("{:.1}G", gb)
    } else if mb >= 1.0 {
        format!("{:.0}M", mb)
    } else if kb >= 1.0 {
        format!("{:.0}K", kb)
    } else {
        format!("{}B", bytes)
    }
}

/// Return the appropriate color for a load-average value based on core count.
pub fn load_avg_color(val: f64, core_count: f64) -> Color {
    let t = theme();
    if val < core_count {
        t.green
    } else if val < 2.0 * core_count {
        t.yellow
    } else {
        t.red
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_str_short_string() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_str_long_string() {
        assert_eq!(truncate_str("hello world", 8), "hello...");
    }

    #[test]
    fn truncate_str_utf8() {
        assert_eq!(truncate_str("caf\u{e9}", 4), "caf\u{e9}");
        assert_eq!(truncate_str("\u{65e5}\u{672c}\u{8a9e}", 3), "\u{65e5}\u{672c}\u{8a9e}");
        assert_eq!(truncate_str("hello\u{4e16}\u{754c}", 6), "hel...");
    }

    #[test]
    fn safe_truncate_short() {
        let s = "hello";
        assert_eq!(safe_truncate(s, 10), "hello");
    }

    #[test]
    fn safe_truncate_utf8_boundary() {
        let s = "caf\u{e9}";
        assert_eq!(safe_truncate(s, 3), "caf");
        assert_eq!(safe_truncate(s, 5), "caf\u{e9}");
    }

    #[test]
    fn format_bytes_rate_units() {
        assert_eq!(format_bytes_rate(500), "500 B/s");
        assert_eq!(format_bytes_rate(2048), "2.00 KB/s");
        assert_eq!(format_bytes_rate(2_097_152), "2.00 MB/s");
    }

    #[test]
    fn progress_bar_empty() {
        assert_eq!(progress_bar(0.0, 10), "[\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}\u{2591}]");
    }

    #[test]
    fn progress_bar_full() {
        assert_eq!(progress_bar(100.0, 5), "[\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}]");
    }

    #[test]
    fn format_number_thousands() {
        assert_eq!(format_number(1234567), "1,234,567");
    }
}
