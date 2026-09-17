//! Bundled palettes selectable by `appearance.theme` beside One Dark. Each is
//! a host-neutral palette JSON under `assets/themes`, resolved through the
//! same path as a user's palette file, so the two never drift.

use super::{HostPalette, Theme};

/// A bundled theme: the id `appearance.theme` names, its label, its palette.
#[derive(Clone, Copy, Debug)]
pub struct Builtin {
    pub id: &'static str,
    pub label: &'static str,
    json: &'static str,
}

macro_rules! builtin {
    ($id:literal, $label:literal) => {
        Builtin {
            id: $id,
            label: $label,
            json: include_str!(concat!("../../assets/themes/", $id, ".json")),
        }
    };
}

/// Every bundled palette, in the order the theme picker lists them after One Dark.
pub const BUILTIN: &[Builtin] = &[
    builtin!("dracula", "Dracula"),
    builtin!("catppuccin-mocha", "Catppuccin Mocha"),
    builtin!("catppuccin-latte", "Catppuccin Latte"),
    builtin!("github-dark", "GitHub Dark"),
    builtin!("github-light", "GitHub Light"),
    builtin!("vscode-dark", "VS Code Dark Modern"),
    builtin!("vscode-light", "VS Code Light Modern"),
];

/// The id of the code-defined default theme.
pub const ONE_DARK: &str = "one-dark";

/// `(id, label)` for One Dark and every bundled palette.
pub fn choices() -> impl Iterator<Item = (&'static str, &'static str)> {
    std::iter::once((ONE_DARK, "One Dark")).chain(BUILTIN.iter().map(|b| (b.id, b.label)))
}

pub fn is_builtin(id: &str) -> bool {
    id == ONE_DARK || BUILTIN.iter().any(|b| b.id == id)
}

impl Theme {
    /// The bundled theme with this id, if any.
    pub fn builtin(id: &str) -> Option<Self> {
        if id == ONE_DARK {
            return Some(Self::one_dark());
        }
        let builtin = BUILTIN.iter().find(|b| b.id == id)?;
        let palette = HostPalette::from_json(builtin.json).expect("bundled palettes are valid");
        Some(Self::from_host(&palette))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Appearance;

    #[test]
    fn every_bundled_palette_resolves_with_its_declared_appearance() {
        let mut ids = std::collections::HashSet::new();
        for builtin in BUILTIN {
            assert!(ids.insert(builtin.id), "duplicate id {}", builtin.id);
            let theme = Theme::builtin(builtin.id).unwrap();
            let light = builtin.id.ends_with("light") || builtin.id.ends_with("latte");
            assert_eq!(
                theme.appearance,
                if light {
                    Appearance::Light
                } else {
                    Appearance::Dark
                },
                "{}",
                builtin.id
            );
            // The palette supplied every surface it names; nothing fell back to
            // the neutral defaults.
            assert_ne!(theme.editor.bg, theme.bar.bg, "{}", builtin.id);
            assert!(is_builtin(builtin.id));
        }
        assert!(Theme::builtin("one-dark").is_some());
        assert!(Theme::builtin("nope").is_none());
        assert_eq!(choices().count(), BUILTIN.len() + 1);
    }
}
