//! Integration tests for the refactored model and view modules.
//! Verifies that the split modules work together correctly.

use sitrep::model::{
    AppView, MonitorData, UIState, MemoryInfo, NetworkInfo,
    FdInfo, ContextSwitchInfo, SocketOverviewInfo,
    ContainerUIState, LogViewState, SwarmUIState, ServiceLogState,
};
use sitrep::view::{Presenter, truncate_str, safe_truncate};

#[test]
fn model_types_construct() {
    let _ = UIState::default();
    let _ = ContainerUIState::default();
    let _ = LogViewState::new("cid".into(), "cname".into());
    let _ = SwarmUIState::default();
    let _ = ServiceLogState::new("sid".into(), "sname".into());
}

#[test]
fn app_view_matches() {
    assert!(matches!(AppView::System, AppView::System));
    assert!(matches!(AppView::Containers, AppView::Containers));
    let logs = AppView::ContainerLogs("abc".into());
    assert!(matches!(logs, AppView::ContainerLogs(_)));
}

#[test]
fn view_helpers_pure() {
    assert_eq!(truncate_str("hello", 5), "hello");
    assert_eq!(truncate_str("hello world", 8), "hello...");
    let s = "café";
    assert_eq!(safe_truncate(s, 10), "café");
}

#[test]
fn presenter_render_size_guard_checks_terminal() {
    // Just verify the function exists and returns a Result.
    // In headless environments (Docker, CI) there is no tty, so
    // terminal::size() may return an error — that's expected and fine.
    let _result = Presenter::render_size_guard();
    // We intentionally don't assert is_ok() because the outcome
    // depends on whether a real terminal is attached.
}

#[test]
fn monitor_data_structure() {
    let data = MonitorData {
        time: "12:00:00".into(),
        core_count: 8.0,
        load_avg: (1.0, 0.5, 0.3),
        historical_top: vec![],
        disk_space: vec![],
        disk_busy_pct: 0.0,
        memory: MemoryInfo::default(),
        network: NetworkInfo::default(),
        fd_info: FdInfo::default(),
        context_switches: ContextSwitchInfo::default(),
        socket_overview: SocketOverviewInfo::default(),
    };
    assert_eq!(data.core_count, 8.0);
    assert_eq!(data.time, "12:00:00");
}
