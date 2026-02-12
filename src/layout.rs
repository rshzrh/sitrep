use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SectionId {
    Summary,
    CpuProcesses,
    DiskIo,
    Network,
    FileDescriptors,
    SocketOverview,
}

impl fmt::Display for SectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SectionId::Summary => write!(f, "System Summary"),
            SectionId::CpuProcesses => write!(f, "Top 5 CPU Processes (Past 1 Minute)"),
            SectionId::DiskIo => write!(f, "Top 5 Disk I/O Processes (Past 1 Minute)"),
            SectionId::Network => write!(f, "Network & Bandwidth"),
            SectionId::FileDescriptors => write!(f, "File Descriptor Details"),
            SectionId::SocketOverview => write!(f, "Socket Details"),
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
                SectionLayout::new(SectionId::Summary),
                SectionLayout::new(SectionId::CpuProcesses),
                SectionLayout::new(SectionId::DiskIo),
                SectionLayout::new(SectionId::Network),
                SectionLayout::new(SectionId::FileDescriptors).collapsed(),
                SectionLayout::new(SectionId::SocketOverview).collapsed(),
            ],
        }
    }

    pub fn toggle_section(&mut self, id: SectionId) {
        if id == SectionId::Summary {
            return;
        }
        if let Some(s) = self.sections.iter_mut().find(|s| s.id == id) {
            s.collapsed = !s.collapsed;
        }
    }

    pub fn is_collapsed(&self, id: SectionId) -> bool {
        self.sections.iter().find(|s| s.id == id).map(|s| s.collapsed).unwrap_or(false)
    }
}
