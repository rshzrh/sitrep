use std::cell::RefCell;
use std::collections::HashSet;
use std::collections::VecDeque;

struct LogSearchCache {
    line_version: u64,
    query: String,
    matches: Vec<usize>,
}

struct MultiLogSearchCache {
    line_version: u64,
    query: String,
    matches: Vec<usize>,
}

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
    pub truncated_count: u64, // number of lines dropped due to buffer cap
    line_version: u64,
    search_cache: RefCell<Option<LogSearchCache>>,
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
            truncated_count: 0,
            line_version: 0,
            search_cache: RefCell::new(None),
        }
    }

    pub fn push_line(&mut self, line: String) {
        if self.lines.len() >= 5000 {
            self.lines.pop_front();
            self.truncated_count += 1;
        }
        self.lines.push_back(line);
        self.line_version += 1;
        *self.search_cache.borrow_mut() = None;
    }

    pub fn with_filtered_indices<R>(&self, f: impl FnOnce(&[usize]) -> R) -> R {
        let query = self.search_query.to_lowercase();
        let mut cache = self.search_cache.borrow_mut();
        let cache_miss = cache
            .as_ref()
            .map(|cached| cached.line_version != self.line_version || cached.query != query)
            .unwrap_or(true);

        if cache_miss {
            let matches = if query.is_empty() {
                (0..self.lines.len()).collect()
            } else {
                self.lines
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, line)| line.to_lowercase().contains(&query).then_some(idx))
                    .collect()
            };
            *cache = Some(LogSearchCache {
                line_version: self.line_version,
                query,
                matches,
            });
        }

        f(&cache.as_ref().expect("search cache populated").matches)
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
    pub truncated_count: u64,
    line_version: u64,
    search_cache: RefCell<Option<MultiLogSearchCache>>,
}

impl MultiLogViewState {
    pub fn new() -> Self {
        Self {
            lines: VecDeque::with_capacity(20000),
            scroll_offset: 0,
            auto_follow: true,
            search_mode: false,
            search_query: String::new(),
            truncated_count: 0,
            line_version: 0,
            search_cache: RefCell::new(None),
        }
    }

    pub fn push_line(&mut self, line: MultiLogLine) {
        if self.lines.len() >= 20000 {
            self.lines.pop_front();
            self.truncated_count += 1;
        }
        self.lines.push_back(line);
        self.line_version += 1;
        *self.search_cache.borrow_mut() = None;
    }

    pub fn with_filtered_indices<R>(&self, f: impl FnOnce(&[usize]) -> R) -> R {
        let query = self.search_query.to_lowercase();
        let mut cache = self.search_cache.borrow_mut();
        let cache_miss = cache
            .as_ref()
            .map(|cached| cached.line_version != self.line_version || cached.query != query)
            .unwrap_or(true);

        if cache_miss {
            let matches = if query.is_empty() {
                (0..self.lines.len()).collect()
            } else {
                self.lines
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, entry)| {
                        entry.line.to_lowercase().contains(&query).then_some(idx)
                    })
                    .collect()
            };
            *cache = Some(MultiLogSearchCache {
                line_version: self.line_version,
                query,
                matches,
            });
        }

        f(&cache.as_ref().expect("multi-log search cache populated").matches)
    }
}

// --- Container UI state ---

pub struct ContainerUIState {
    pub selected_index: usize,
    pub selected_id: Option<String>,
    pub total_rows: usize,
    pub expanded_ids: HashSet<String>,
    pub selected_containers: HashSet<String>,
}

impl Default for ContainerUIState {
    fn default() -> Self {
        Self {
            selected_index: 0,
            selected_id: None,
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
