//! The platform's settings surfaces: the sections the Settings app shows, and
//! the shell's search indexes.
//!
//! WHY this catalogue lives in the SDK and not in the Settings app: the
//! Settings app renders these sections, and the launcher's search has to
//! offer the same ones. If each side held its own list, the two would drift,
//! and the drift would show up as a search result that opens a section the
//! Settings app does not have -- a destination that does not exist. One
//! definition, in the platform, is the only arrangement that cannot drift:
//! the Settings app renders these sections and the shell searches these
//! sections, and both read this table.
//!
//! WHY the keywords are here too: a settings section is found by the words a
//! person would use for it, not only by the label the designer chose. "wifi"
//! has to reach the Network section even though the section is called
//! "Network", and the words that reach it are a property of the section, so
//! they live beside it.

/// One settings surface of the platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Device = 0,
    Network = 1,
    Personal = 2,
    Apps = 3,
    Power = 4,
    Security = 5,
    System = 6,
}

impl Section {
    /// How many settings surfaces the platform has.
    pub const COUNT: usize = 7;

    /// Every section, in landing-grid order.
    pub const ALL: [Section; Section::COUNT] = [
        Section::Device,
        Section::Network,
        Section::Personal,
        Section::Apps,
        Section::Power,
        Section::Security,
        Section::System,
    ];

    /// The section at `index`, if there is one.
    pub const fn from_index(index: usize) -> Option<Section> {
        match index {
            0 => Some(Section::Device),
            1 => Some(Section::Network),
            2 => Some(Section::Personal),
            3 => Some(Section::Apps),
            4 => Some(Section::Power),
            5 => Some(Section::Security),
            6 => Some(Section::System),
            _ => None,
        }
    }

    /// The section's position in the landing grid.
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The title the Settings app puts on the section's detail screen.
    pub const fn title(self) -> &'static [u8] {
        match self {
            Section::Device => b"Device",
            Section::Network => b"Network",
            Section::Personal => b"Personal",
            Section::Apps => b"Apps & Storage",
            Section::Power => b"Power & Battery",
            Section::Security => b"Security & Privacy",
            Section::System => b"About InterOS",
        }
    }

    /// The short name the section is known by in a list.
    pub const fn short_title(self) -> &'static [u8] {
        match self {
            Section::Device => b"Device",
            Section::Network => b"Network",
            Section::Personal => b"Personal",
            Section::Apps => b"Apps",
            Section::Power => b"Power",
            Section::Security => b"Security",
            Section::System => b"System",
        }
    }

    /// The words a person would use to reach this section.
    ///
    /// WHY these are part of the section and not of the search: a search that
    /// only matches labels cannot answer "wifi" or "battery", and the person
    /// typing does not know which label the designer chose. The words belong
    /// to the surface, so they are declared with it.
    pub const fn keywords(self) -> &'static [&'static [u8]] {
        match self {
            Section::Device => &[
                b"device",
                b"display",
                b"screen",
                b"brightness",
                b"volume",
                b"sound",
                b"theme",
                b"wallpaper",
                b"date",
                b"time",
            ],
            Section::Network => &[
                b"network",
                b"wifi",
                b"wi-fi",
                b"wireless",
                b"bluetooth",
                b"cellular",
                b"data",
                b"hotspot",
                b"airplane",
                b"mobile",
                b"internet",
            ],
            Section::Personal => &[
                b"personal",
                b"user",
                b"profile",
                b"name",
                b"language",
                b"wallpaper",
                b"theme",
                b"owner",
                b"account",
            ],
            Section::Apps => &[
                b"apps",
                b"applications",
                b"storage",
                b"files",
                b"downloads",
                b"permissions",
                b"default apps",
                b"packages",
            ],
            Section::Power => &[
                b"power",
                b"battery",
                b"energy",
                b"charging",
                b"sleep",
                b"performance",
                b"governor",
                b"power save",
            ],
            Section::Security => &[
                b"security",
                b"privacy",
                b"pin",
                b"lock",
                b"password",
                b"biometric",
                b"permission",
                b"keystore",
                b"vault",
            ],
            Section::System => &[
                b"system",
                b"about",
                b"version",
                b"update",
                b"developer",
                b"logs",
                b"reset",
                b"support",
                b"interos",
            ],
        }
    }

    /// The section a title names, if it names one.
    ///
    /// WHY both spellings: the detail screen is titled "Apps & Storage" and
    /// the landing grid calls the same section "Apps", so an answer that
    /// recognised only one of them would resolve one of the two names and
    /// leave the other as a destination that does not exist.
    pub fn from_title(title: &[u8]) -> Option<Section> {
        let mut index = 0usize;
        while index < Section::COUNT {
            let section = Section::from_index(index)?;
            if section.title().eq_ignore_ascii_case(title)
                || section.short_title().eq_ignore_ascii_case(title)
            {
                return Some(section);
            }
            index += 1;
        }
        None
    }
}
