use std::collections::HashSet;
use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct DockerContainerInfo {
    pub id: String,         // short ID (first 12 chars)
    pub name: String,       // container name
    pub image: String,      // image name (shown when expanded)
    pub status: String,     // "running", "paused", etc.
    pub state: String,      // raw state string from Docker
    pub uptime: String,     // human-readable (e.g. "2h 34m")
    pub cpu_percent: f64,   // from stats
    pub ports: String,      // e.g. "0.0.0.0:8080->80/tcp"
    pub ip_address: String, // internal IP from NetworkSettings
}

// --- Log viewer state ---

pub struct LogViewState {
    pub container_id: String,
    pub container_name: String,
    pub lines: VecDeque<String>,
    pub scroll_offset: usize, // 0 = at bottom (following)
    pub auto_follow: bool,
    pub search_mode: bool,    // true when typing a search query
    pub search_query: String, // current search text
}

impl LogViewState {
    pub fn new(container_id: String, container_name: String) -> Self {
        Self {
            container_id,
            container_name,
            lines: VecDeque::with_capacity(5000),
            scroll_offset: 0,
            auto_follow: true,
            search_mode: false,
            search_query: String::new(),
        }
    }

    pub fn push_line(&mut self, line: String) {
        if self.lines.len() >= 5000 {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }
}

#[derive(Clone, Debug)]
pub struct MultiLogLine {
    pub container_id: String,
    pub container_name: String,
    pub line: String,
    pub seq: u64,
}

pub struct MultiLogViewState {
    pub lines: VecDeque<MultiLogLine>,
    pub scroll_offset: usize, // 0 = at bottom (following)
    pub auto_follow: bool,
    pub search_mode: bool,
    pub search_query: String,
}

impl MultiLogViewState {
    pub fn new() -> Self {
        Self {
            lines: VecDeque::with_capacity(20000),
            scroll_offset: 0,
            auto_follow: true,
            search_mode: false,
            search_query: String::new(),
        }
    }

    pub fn push_line(&mut self, line: MultiLogLine) {
        if self.lines.len() >= 20000 {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }
}

// --- Container UI state ---

pub struct ContainerUIState {
    pub selected_index: usize,
    pub total_rows: usize,
    pub expanded_ids: HashSet<String>,
    pub selected_containers: HashSet<String>,
}

impl Default for ContainerUIState {
    fn default() -> Self {
        Self {
            selected_index: 0,
            total_rows: 0,
            expanded_ids: HashSet::new(),
            selected_containers: HashSet::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_view_state_push_line_caps_at_5000() {
        let mut state = LogViewState::new("abc123".into(), "my-container".into());
        for i in 0..5010 {
            state.push_line(format!("line {}", i));
        }
        assert_eq!(state.lines.len(), 5000);
        assert_eq!(state.lines.front(), Some(&"line 10".to_string()));
        assert_eq!(state.lines.back(), Some(&"line 5009".to_string()));
    }

    #[test]
    fn container_ui_state_default() {
        let state = ContainerUIState::default();
        assert_eq!(state.selected_index, 0);
        assert_eq!(state.total_rows, 0);
        assert!(state.expanded_ids.is_empty());
        assert!(state.selected_containers.is_empty());
    }

    #[test]
    fn multi_log_view_state_push_line_caps_at_20000() {
        let mut state = MultiLogViewState::new();
        for i in 0..20010 {
            state.push_line(MultiLogLine {
                container_id: "abc123".into(),
                container_name: "my-container".into(),
                line: format!("line {}", i),
                seq: i as u64,
            });
        }
        assert_eq!(state.lines.len(), 20000);
        assert_eq!(state.lines.front().map(|l| l.seq), Some(10));
        assert_eq!(state.lines.back().map(|l| l.seq), Some(20009));
    }
}
