use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SectionId {
    DiskSpace,
    Memory,
    LoadAverage,
    CpuProcesses,
    DiskIo,
    Network,
    FileDescriptors,
    ContextSwitches,
    SocketOverview,
}

impl fmt::Display for SectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SectionId::DiskSpace => write!(f, "Disk Space Warnings"),
            SectionId::Memory => write!(f, "Memory Overview"),
            SectionId::LoadAverage => write!(f, "Load Average"),
            SectionId::CpuProcesses => write!(f, "Top 5 CPU Processes (Past 1 Minute)"),
            SectionId::DiskIo => write!(f, "Top 5 Disk I/O Processes (Past 1 Minute)"),
            SectionId::Network => write!(f, "Network & Bandwidth"),
            SectionId::FileDescriptors => write!(f, "Open File Descriptors"),
            SectionId::ContextSwitches => write!(f, "Context Switches"),
            SectionId::SocketOverview => write!(f, "TCP/Socket Overview"),
        }
    }
}

pub struct SectionLayout {
    pub id: SectionId,
    pub title: String,
    pub collapsed: bool,
}

impl SectionLayout {
    pub fn new(id: SectionId) -> Self {
        Self {
            title: id.to_string(),
            id,
            collapsed: false,
        }
    }

    pub fn collapsed(mut self) -> Self {
        self.collapsed = true;
        self
    }
}

pub struct Layout {
    pub sections: Vec<SectionLayout>,
}

impl Layout {
    /// Default section ordering for the diagnostics report.
    pub fn default_layout() -> Self {
        Self {
            sections: vec![
                SectionLayout::new(SectionId::LoadAverage),
                SectionLayout::new(SectionId::DiskSpace),
                SectionLayout::new(SectionId::Memory),
                SectionLayout::new(SectionId::CpuProcesses),
                SectionLayout::new(SectionId::DiskIo),
                SectionLayout::new(SectionId::Network),
                SectionLayout::new(SectionId::FileDescriptors),
                SectionLayout::new(SectionId::ContextSwitches).collapsed(),
                SectionLayout::new(SectionId::SocketOverview).collapsed(),
            ],
        }
    }

    pub fn toggle_section(&mut self, id: SectionId) {
        if let Some(s) = self.sections.iter_mut().find(|s| s.id == id) {
            s.collapsed = !s.collapsed;
        }
    }

    pub fn is_collapsed(&self, id: SectionId) -> bool {
        self.sections.iter().find(|s| s.id == id).map(|s| s.collapsed).unwrap_or(false)
    }
}
