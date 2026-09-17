//! The single icon set (Lucide) and the bundled fonts, compiled in so every
//! frontend renders identically. Adding an icon: drop the SVG in
//! `assets/icons` and add a variant here.

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum IconName {
    Activity,
    AudioWaveform,
    Binary,
    Box,
    Braces,
    ChevronDown,
    ChevronRight,
    ChevronsLeft,
    ChevronsRight,
    CircleDot,
    Folder,
    FolderOpen,
    ListTree,
    LoaderCircle,
    Locate,
    PanelLeft,
    Plus,
    Search,
    Settings,
    Sigma,
    TriangleAlert,
    Type,
    WindowClose,
    WindowMaximize,
    WindowMinimize,
    WindowRestore,
    X,
}

macro_rules! icons {
    ($($variant:ident => $name:literal),* $(,)?) => {
        impl IconName {
            pub const ALL: &'static [IconName] = &[$(IconName::$variant),*];

            /// Asset path, e.g. `icons/plus.svg`.
            pub fn path(self) -> &'static str {
                match self { $(IconName::$variant => concat!("icons/", $name, ".svg")),* }
            }

            /// The SVG source of the icon.
            pub fn svg(self) -> &'static [u8] {
                match self {
                    $(IconName::$variant => include_bytes!(concat!("../assets/icons/", $name, ".svg"))),*
                }
            }
        }
    };
}

icons!(
    Activity => "activity",
    AudioWaveform => "audio-waveform",
    Binary => "binary",
    Box => "box",
    Braces => "braces",
    ChevronDown => "chevron-down",
    ChevronRight => "chevron-right",
    ChevronsLeft => "chevrons-left",
    ChevronsRight => "chevrons-right",
    CircleDot => "circle-dot",
    Folder => "folder",
    FolderOpen => "folder-open",
    ListTree => "list-tree",
    LoaderCircle => "loader-circle",
    Locate => "locate",
    PanelLeft => "panel-left",
    Plus => "plus",
    Search => "search",
    Settings => "settings",
    Sigma => "sigma",
    TriangleAlert => "triangle-alert",
    Type => "type",
    WindowClose => "window-close",
    WindowMaximize => "window-maximize",
    WindowMinimize => "window-minimize",
    WindowRestore => "window-restore",
    X => "x",
);

impl IconName {
    /// Look an icon up by asset path.
    pub fn from_path(path: &str) -> Option<IconName> {
        IconName::ALL.iter().copied().find(|i| i.path() == path)
    }
}

/// A bundled font face.
#[derive(Clone, Copy, Debug)]
pub struct FontFace {
    pub family: &'static str,
    pub weight: u16,
    pub bytes: &'static [u8],
}

/// IBM Plex Sans (UI) and Lilex (mono), the same faces on every platform.
pub const FONTS: &[FontFace] = &[
    FontFace {
        family: "IBM Plex Sans",
        weight: 400,
        bytes: include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf"),
    },
    FontFace {
        family: "IBM Plex Sans",
        weight: 600,
        bytes: include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf"),
    },
    FontFace {
        family: "Lilex",
        weight: 400,
        bytes: include_bytes!("../assets/fonts/Lilex-Regular.ttf"),
    },
];
