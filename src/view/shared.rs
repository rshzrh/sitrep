use std::io::{self, Write};
use crossterm::{queue, style::{Color, SetForegroundColor, SetBackgroundColor, ResetColor, Attribute, SetAttribute}};

/// Truncate a string to at most `max_len` characters (not bytes), appending "..."
/// if truncated. Safe for multi-byte UTF-8.
pub fn truncate_str(s: &str, max_len: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_len {
        s.to_string()
    } else {
        let keep = max_len.saturating_sub(3);
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

pub fn write_section_header(out: &mut impl Write, text: &str, selected: bool) -> io::Result<()> {
    if selected {
        queue!(io::stdout(), SetBackgroundColor(Color::DarkBlue), SetForegroundColor(Color::White))?;
    } else {
        queue!(io::stdout(), SetAttribute(Attribute::Bold))?;
    }
    write!(out, "{}\r\n", text)?;
    queue!(io::stdout(), ResetColor, SetAttribute(Attribute::Reset))?;
    Ok(())
}

pub fn write_selectable(out: &mut impl Write, text: &str, selected: bool) -> io::Result<()> {
    if selected {
        queue!(io::stdout(), SetBackgroundColor(Color::DarkGrey), SetForegroundColor(Color::White))?;
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

pub fn progress_bar(percent: f64, width: usize) -> String {
    let filled = ((percent / 100.0) * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}

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
        assert_eq!(truncate_str("café", 4), "café");
        // "日本語" has 3 chars; max_len 3 returns full, max_len 2 gives keep=0 so "..."
        assert_eq!(truncate_str("日本語", 3), "日本語");
        assert_eq!(truncate_str("hello世界", 6), "hel..."); // 7 chars, keep=3
    }

    #[test]
    fn safe_truncate_short() {
        let s = "hello";
        assert_eq!(safe_truncate(s, 10), "hello");
    }

    #[test]
    fn safe_truncate_utf8_boundary() {
        // "café" = c(1) a(1) f(1) é(2) bytes. At 3 bytes, index 3 is start of é, so we get "caf"
        let s = "café";
        assert_eq!(safe_truncate(s, 3), "caf");
        assert_eq!(safe_truncate(s, 5), "café");
    }

    #[test]
    fn format_bytes_rate_units() {
        assert_eq!(format_bytes_rate(500), "500 B/s");
        assert_eq!(format_bytes_rate(2048), "2.00 KB/s");
        assert_eq!(format_bytes_rate(2_097_152), "2.00 MB/s");
    }

    #[test]
    fn progress_bar_empty() {
        assert_eq!(progress_bar(0.0, 10), "[░░░░░░░░░░]");
    }

    #[test]
    fn progress_bar_full() {
        assert_eq!(progress_bar(100.0, 5), "[█████]");
    }

    #[test]
    fn format_number_thousands() {
        assert_eq!(format_number(1234567), "1,234,567");
    }
}
