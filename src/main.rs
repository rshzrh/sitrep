mod model;
mod view;
mod controller;
mod layout;

use std::io;
use std::time::{Duration, Instant};
use crossterm::{
    execute,
    event::{self, Event, KeyCode, KeyModifiers, KeyEvent},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, Clear, ClearType},
};
use controller::Monitor;
use view::{Presenter, RowKind};
use sysinfo::Pid;

fn main() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, Clear(ClearType::All))?;

    let mut monitor = Monitor::new();
    let tick_rate = Duration::from_secs(3);
    let mut last_tick = Instant::now() - tick_rate;
    let mut row_mapping: Vec<(Pid, RowKind)> = Vec::new();
    let mut needs_render = true;

    loop {
        let now = Instant::now();
        if now.duration_since(last_tick) >= tick_rate {
            monitor.update();
            last_tick = now;
            needs_render = true;
        }

        if needs_render {
            if let Some(ref data) = monitor.last_data {
                row_mapping = Presenter::render(data, &mut monitor.ui_state, &monitor.layout)?;
            }
            needs_render = false;
        }

        let timeout = tick_rate.saturating_sub(now.elapsed());
        if event::poll(timeout.min(Duration::from_millis(100)))? {
            if let Event::Key(KeyEvent { code, modifiers, .. }) = event::read()? {
                if code == KeyCode::Char('c') && modifiers.contains(KeyModifiers::CONTROL) {
                    break;
                }
                match code {
                    KeyCode::Char('q') | KeyCode::Esc => break,
                    KeyCode::Up => {
                        if monitor.ui_state.selected_index > 0 {
                            monitor.ui_state.selected_index -= 1;
                            needs_render = true;
                        }
                    }
                    KeyCode::Down => {
                        if monitor.ui_state.selected_index + 1 < monitor.ui_state.total_rows {
                            monitor.ui_state.selected_index += 1;
                            needs_render = true;
                        }
                    }
                    KeyCode::Right => {
                        if monitor.ui_state.selected_index < row_mapping.len() {
                            let (pid, kind) = row_mapping[monitor.ui_state.selected_index];
                            match kind {
                                RowKind::SectionHeader(section_id) => {
                                    // Expand (uncollapse) section
                                    if monitor.layout.is_collapsed(section_id) {
                                        monitor.layout.toggle_section(section_id);
                                        needs_render = true;
                                    }
                                }
                                RowKind::CpuParent => {
                                    monitor.ui_state.cpu_expanded_pids.insert(pid);
                                    needs_render = true;
                                }
                                RowKind::DiskParent => {
                                    monitor.ui_state.disk_expanded_pids.insert(pid);
                                    needs_render = true;
                                }
                                _ => {}
                            }
                        }
                    }
                    KeyCode::Left => {
                        if monitor.ui_state.selected_index < row_mapping.len() {
                            let (pid, kind) = row_mapping[monitor.ui_state.selected_index];
                            match kind {
                                RowKind::SectionHeader(section_id) => {
                                    // Collapse section
                                    if !monitor.layout.is_collapsed(section_id) {
                                        monitor.layout.toggle_section(section_id);
                                        needs_render = true;
                                    }
                                }
                                RowKind::CpuParent => {
                                    monitor.ui_state.cpu_expanded_pids.remove(&pid);
                                    needs_render = true;
                                }
                                RowKind::DiskParent => {
                                    monitor.ui_state.disk_expanded_pids.remove(&pid);
                                    needs_render = true;
                                }
                                RowKind::CpuChild => {
                                    // Find parent and collapse
                                    let mut idx = monitor.ui_state.selected_index;
                                    while idx > 0 {
                                        idx -= 1;
                                        if row_mapping[idx].1 == RowKind::CpuParent {
                                            monitor.ui_state.cpu_expanded_pids.remove(&row_mapping[idx].0);
                                            monitor.ui_state.selected_index = idx;
                                            needs_render = true;
                                            break;
                                        }
                                    }
                                }
                                RowKind::DiskChild => {
                                    let mut idx = monitor.ui_state.selected_index;
                                    while idx > 0 {
                                        idx -= 1;
                                        if row_mapping[idx].1 == RowKind::DiskParent {
                                            monitor.ui_state.disk_expanded_pids.remove(&row_mapping[idx].0);
                                            monitor.ui_state.selected_index = idx;
                                            needs_render = true;
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    execute!(stdout, LeaveAlternateScreen)?;
    disable_raw_mode()?;
    Ok(())
}
