//! The static settings table. Ids are lowercase dotted paths with camelCase
//! leaves, so `waves.animation` is `volna.waves.animation` inside VS Code.
//! Titles, descriptions and keywords live here; frontends never invent text.

use super::Value;

/// The host the viewer runs on. Entries and enum members can be limited to hosts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Host {
    Native,
    Web,
    Vscode,
}

/// A set of hosts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hosts(u8);

impl Hosts {
    pub const NATIVE: Hosts = Hosts(1);
    pub const WEB: Hosts = Hosts(2);
    pub const VSCODE: Hosts = Hosts(4);
    pub const ALL: Hosts = Hosts(7);
    pub const NATIVE_WEB: Hosts = Hosts(3);

    pub fn contains(self, host: Host) -> bool {
        let bit = match host {
            Host::Native => 1,
            Host::Web => 2,
            Host::Vscode => 4,
        };
        self.0 & bit != 0
    }
}

/// How a change takes effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Apply {
    Live,
    /// The value is read when a trace is opened.
    OnReopen,
    Restart,
}

impl Apply {
    /// The badge shown on an item, if any.
    pub fn badge(self) -> Option<&'static str> {
        match self {
            Apply::Live => None,
            Apply::OnReopen => Some("reopen trace to apply"),
            Apply::Restart => Some("restart to apply"),
        }
    }
}

/// A page of the settings editor, in display order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Page {
    Appearance,
    Waves,
    Workspace,
    Remote,
}

impl Page {
    pub const ALL: &'static [Page] =
        &[Page::Appearance, Page::Waves, Page::Workspace, Page::Remote];

    pub fn id(self) -> &'static str {
        match self {
            Page::Appearance => "appearance",
            Page::Waves => "waves",
            Page::Workspace => "workspace",
            Page::Remote => "remote",
        }
    }
    pub fn title(self) -> &'static str {
        match self {
            Page::Appearance => "Appearance",
            Page::Waves => "Waves",
            Page::Workspace => "Workspace",
            Page::Remote => "Remote",
        }
    }
    pub fn description(self) -> &'static str {
        match self {
            Page::Appearance => "Theme and chrome.",
            Page::Waves => "Waveform panels and navigation.",
            Page::Workspace => "How sessions are saved beside traces.",
            Page::Remote => "Limits and the server for traces on a remote host.",
        }
    }
    pub fn icon(self) -> crate::icons::IconName {
        use crate::icons::IconName;
        match self {
            Page::Appearance => IconName::Type,
            Page::Waves => IconName::AudioWaveform,
            Page::Workspace => IconName::Folder,
            Page::Remote => IconName::Box,
        }
    }
}

/// One member of an enum setting. `hosts` limits where it is offered.
#[derive(Clone, Copy, Debug)]
pub struct Choice {
    pub value: &'static str,
    pub label: &'static str,
    pub hosts: Hosts,
}

const fn choice(value: &'static str, label: &'static str) -> Choice {
    Choice {
        value,
        label,
        hosts: Hosts::ALL,
    }
}

/// The type and constraints of a setting.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Bool,
    Integer {
        min: i64,
        max: i64,
        step: i64,
    },
    Number {
        min: f64,
        max: f64,
        step: f64,
    },
    Text,
    Enum(&'static [Choice]),
    /// `one-dark` plus the palette names the host found; validated by the store.
    Theme,
}

/// A constant default; [`Literal::value`] turns it into a [`Value`].
#[derive(Clone, Copy, Debug)]
pub enum Literal {
    Bool(bool),
    Integer(i64),
    Number(f64),
    Text(&'static str),
}

impl Literal {
    pub fn value(self) -> Value {
        match self {
            Literal::Bool(b) => Value::Bool(b),
            Literal::Integer(i) => Value::Integer(i),
            Literal::Number(n) => Value::Number(n),
            Literal::Text(s) => Value::Text(s.to_owned()),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub id: &'static str,
    pub page: Page,
    pub group: &'static str,
    pub title: &'static str,
    /// Markdown allowed (inline code only is rendered natively).
    pub description: &'static str,
    pub keywords: &'static [&'static str],
    pub kind: Kind,
    pub default: Literal,
    pub apply: Apply,
    pub hosts: Hosts,
    /// The settings file version that introduced this key.
    pub since: u32,
}

impl Spec {
    pub fn available(&self, host: Host) -> bool {
        self.hosts.contains(host)
    }

    /// Enum members offered on `host`.
    pub fn choices(&self, host: Host) -> Vec<Choice> {
        match self.kind {
            Kind::Enum(choices) => choices
                .iter()
                .copied()
                .filter(|c| c.hosts.contains(host))
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Check a value against this entry on `host`; `themes` are the palette
    /// names the host offers. Returns the problem the diagnostic reports.
    pub fn validate(&self, value: &Value, host: Host, themes: &[String]) -> Result<(), String> {
        match (self.kind, value) {
            (Kind::Bool, Value::Bool(_)) => Ok(()),
            (Kind::Bool, _) => Err("expected true or false".into()),
            (Kind::Integer { min, max, .. }, Value::Integer(v)) => {
                if (min..=max).contains(v) {
                    Ok(())
                } else {
                    Err(format!("expected {min}–{max}"))
                }
            }
            (Kind::Integer { .. }, _) => Err("expected an integer".into()),
            (Kind::Number { min, max, .. }, Value::Number(_) | Value::Integer(_)) => {
                let v = value.as_f64().unwrap();
                if v.is_finite() && (min..=max).contains(&v) {
                    Ok(())
                } else {
                    Err(format!("expected {min}–{max}"))
                }
            }
            (Kind::Number { .. }, _) => Err("expected a number".into()),
            (Kind::Text, Value::Text(_)) => Ok(()),
            (Kind::Text, _) => Err("expected a string".into()),
            (Kind::Enum(_), Value::Text(v)) => {
                let choices = self.choices(host);
                if choices.iter().any(|c| c.value == v) {
                    Ok(())
                } else {
                    Err(format!(
                        "expected one of {}",
                        choices
                            .iter()
                            .map(|c| format!("\"{}\"", c.value))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                }
            }
            (Kind::Enum(_), _) => Err("expected a string".into()),
            (Kind::Theme, Value::Text(v)) => {
                if v == "one-dark" || themes.iter().any(|t| t == v) {
                    Ok(())
                } else {
                    Err(format!("unknown theme \"{v}\""))
                }
            }
            (Kind::Theme, _) => Err("expected a string".into()),
        }
    }

    /// The JSON Schema fragment of this entry (also the VS Code property body).
    pub fn schema(&self, host: Option<Host>) -> serde_json::Value {
        use serde_json::json;
        let mut out = match self.kind {
            Kind::Bool => json!({"type": "boolean"}),
            Kind::Integer { min, max, .. } => {
                json!({"type": "integer", "minimum": min, "maximum": max})
            }
            Kind::Number { min, max, .. } => {
                json!({"type": "number", "minimum": min, "maximum": max})
            }
            Kind::Text | Kind::Theme => json!({"type": "string"}),
            Kind::Enum(choices) => {
                let choices: Vec<_> = choices
                    .iter()
                    .filter(|c| host.is_none_or(|h| c.hosts.contains(h)))
                    .collect();
                json!({
                    "type": "string",
                    "enum": choices.iter().map(|c| c.value).collect::<Vec<_>>(),
                    "enumDescriptions": choices.iter().map(|c| c.label).collect::<Vec<_>>(),
                })
            }
        };
        let object = out.as_object_mut().unwrap();
        object.insert("default".into(), self.default.value().to_json());
        let description = match self.apply {
            Apply::Live => self.description.to_owned(),
            Apply::OnReopen => format!("{} Reopen the trace to apply.", self.description),
            Apply::Restart => format!("{} Restart to apply.", self.description),
        };
        object.insert("markdownDescription".into(), description.into());
        out
    }
}

/// Every setting, in the order the editor shows them.
pub static REGISTRY: &[Spec] = &[
    Spec {
        id: "appearance.theme",
        page: Page::Appearance,
        group: "Theme",
        title: "Theme",
        description: "One Dark, or any palette JSON dropped into the `themes` directory beside `settings.json`. Palettes reload when the file changes.",
        keywords: &["dark", "light", "palette", "colors", "colours"],
        kind: Kind::Theme,
        default: Literal::Text("one-dark"),
        apply: Apply::Live,
        hosts: Hosts::NATIVE_WEB,
        since: 1,
    },
    Spec {
        id: "panels.linkByDefault",
        page: Page::Waves,
        group: "Panels",
        title: "Link new panels",
        description: "New waveform panels follow the shared viewport and cursor. Each panel can still unlink from its header.",
        keywords: &["follow", "cursor", "viewport", "sync", "split"],
        kind: Kind::Bool,
        default: Literal::Bool(true),
        apply: Apply::Live,
        hosts: Hosts::ALL,
        since: 1,
    },
    Spec {
        id: "waves.animation",
        page: Page::Waves,
        group: "Navigation",
        title: "Animation",
        description: "Animate pan and zoom. `reduced` keeps the motion but shortens it; `off` jumps.",
        keywords: &["smooth", "motion", "reduce", "transition"],
        kind: Kind::Enum(&[
            choice("on", "On"),
            choice("reduced", "Reduced"),
            choice("off", "Off"),
        ]),
        default: Literal::Text("on"),
        apply: Apply::Live,
        hosts: Hosts::ALL,
        since: 1,
    },
    Spec {
        id: "waves.snapPixels",
        page: Page::Waves,
        group: "Navigation",
        title: "Snap distance",
        description: "Clicks in the waves snap the cursor to an edge within this many pixels. 0 disables snapping.",
        keywords: &["edge", "cursor", "magnet", "px"],
        kind: Kind::Integer {
            min: 0,
            max: 24,
            step: 1,
        },
        default: Literal::Integer(6),
        apply: Apply::Live,
        hosts: Hosts::ALL,
        since: 1,
    },
    Spec {
        id: "workspace.autosave",
        page: Page::Workspace,
        group: "Persistence",
        title: "Autosave workspace",
        description: "Save the session beside the trace as `<trace>.volna.json`, in VS Code workspace storage, or disable workspace persistence.",
        keywords: &["sidecar", "session", "persist", "save"],
        kind: Kind::Enum(&[
            choice("sidecar", "Beside the trace"),
            Choice {
                value: "vscode",
                label: "VS Code workspace storage",
                hosts: Hosts::VSCODE,
            },
            choice("off", "Off"),
        ]),
        default: Literal::Text("sidecar"),
        apply: Apply::OnReopen,
        hosts: Hosts::ALL,
        since: 1,
    },
    Spec {
        id: "workspace.recentLimit",
        page: Page::Workspace,
        group: "History",
        title: "Recent items to keep",
        description: "How many recent traces and workspaces are remembered.",
        keywords: &["history", "mru", "menu"],
        kind: Kind::Integer {
            min: 5,
            max: 50,
            step: 1,
        },
        default: Literal::Integer(20),
        apply: Apply::Live,
        hosts: Hosts::NATIVE_WEB,
        since: 1,
    },
    Spec {
        id: "remote.memoryMiB",
        page: Page::Remote,
        group: "Limits",
        title: "Memory budget",
        description: "Client admission budget for loaded trace data and decoding workspace, in MiB. This does not bound total browser memory.",
        keywords: &["budget", "limit", "admission", "ram"],
        kind: Kind::Integer {
            min: 1,
            max: 16384,
            step: 64,
        },
        default: Literal::Integer(512),
        apply: Apply::OnReopen,
        hosts: Hosts::VSCODE,
        since: 1,
    },
    Spec {
        id: "remote.objectMiB",
        page: Page::Remote,
        group: "Limits",
        title: "Object size limit",
        description: "Maximum decoded wire size of one complete metadata, signal or track object, in MiB.",
        keywords: &["limit", "signal", "track", "size"],
        kind: Kind::Integer {
            min: 1,
            max: 16384,
            step: 64,
        },
        default: Literal::Integer(256),
        apply: Apply::OnReopen,
        hosts: Hosts::VSCODE,
        since: 1,
    },
    Spec {
        id: "remote.serverPath",
        page: Page::Remote,
        group: "Server",
        title: "Server path",
        description: "Path to `volna-server` on the workspace host. Empty uses the binary bundled with the extension.",
        keywords: &["binary", "executable", "ssh", "container"],
        kind: Kind::Text,
        default: Literal::Text(""),
        apply: Apply::OnReopen,
        hosts: Hosts::VSCODE,
        since: 1,
    },
];

/// The entry with this id.
pub fn spec(id: &str) -> Option<&'static Spec> {
    REGISTRY.iter().find(|s| s.id == id)
}

/// Keys the store recognises but does not register (tooling metadata).
pub const META_KEYS: &[&str] = &["$schema"];

/// Renamed ids: the store rewrites the old name to the new one once.
pub const RENAMED: &[(&str, &str)] = &[("serverPath", "remote.serverPath")];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_dotted_paths_and_defaults_satisfy_their_constraints() {
        let mut seen = std::collections::HashSet::new();
        for spec in REGISTRY {
            assert!(seen.insert(spec.id), "duplicate id {}", spec.id);
            assert!(spec.id.contains('.'), "{} is not a dotted path", spec.id);
            assert!(
                spec.id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.'),
                "{} has unexpected characters",
                spec.id
            );
            assert!(spec.id.chars().next().unwrap().is_ascii_lowercase());
            for host in [Host::Native, Host::Web, Host::Vscode] {
                if spec.available(host) {
                    spec.validate(&spec.default.value(), host, &[])
                        .unwrap_or_else(|e| panic!("{} default: {e}", spec.id));
                }
            }
            assert!(!spec.keywords.is_empty(), "{} has no keywords", spec.id);
        }
        assert_eq!(super::super::Settings::default().waves.snap_pixels, 6);
    }

    #[test]
    fn enum_members_are_filtered_per_host() {
        let autosave = spec("workspace.autosave").unwrap();
        assert!(
            autosave
                .validate(&"vscode".into(), Host::Vscode, &[])
                .is_ok()
        );
        assert!(
            autosave
                .validate(&"vscode".into(), Host::Native, &[])
                .is_err()
        );
        assert_eq!(autosave.choices(Host::Native).len(), 2);
        let theme = spec("appearance.theme").unwrap();
        assert!(
            theme
                .validate(&"gruvbox".into(), Host::Native, &[])
                .is_err()
        );
        assert!(
            theme
                .validate(&"gruvbox".into(), Host::Native, &["gruvbox".into()])
                .is_ok()
        );
    }
}
