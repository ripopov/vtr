//! Themes selectable by `appearance.theme`: the Volna themes (the default),
//! One Dark, and bundled palettes. Each bundled palette is a host-neutral
//! palette JSON under `assets/themes`, resolved through the same path as a
//! user's palette file, so the two never drift.

use super::{HostPalette, Theme, Themes};

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

/// The default: Volna Dark or Volna Light, following the system.
pub const VOLNA: &str = "volna";
pub const VOLNA_DARK: &str = "volna-dark";
pub const VOLNA_LIGHT: &str = "volna-light";
/// Volna Dark for the panels drawn against time in a Volna Light window,
/// whatever the system appearance.
pub const VOLNA_MIXED: &str = "volna-mixed";
/// The code-defined theme that was the default before Volna.
pub const ONE_DARK: &str = "one-dark";

/// The themes defined in code, in the order the theme picker lists them.
const CODED: [(&str, &str); 5] = [
    (VOLNA, "Volna"),
    (VOLNA_DARK, "Volna Dark"),
    (VOLNA_LIGHT, "Volna Light"),
    (VOLNA_MIXED, "Volna Mixed"),
    (ONE_DARK, "One Dark"),
];

/// `(id, label)` for the coded themes and every bundled palette.
pub fn choices() -> impl Iterator<Item = (&'static str, &'static str)> {
    CODED
        .into_iter()
        .chain(BUILTIN.iter().map(|b| (b.id, b.label)))
}

pub fn is_builtin(id: &str) -> bool {
    choices().any(|(known, _)| known == id)
}

/// The themes `id` names while the system appearance is dark (`system_dark`)
/// or light; `None` for an id that is not built in.
pub fn resolve(id: &str, system_dark: bool) -> Option<Themes> {
    let volna = Theme::volna;
    Some(match id {
        VOLNA => Themes::uniform(volna(system_dark)),
        VOLNA_DARK => Themes::uniform(volna(true)),
        VOLNA_LIGHT => Themes::uniform(volna(false)),
        VOLNA_MIXED => Themes {
            chrome: volna(false),
            canvas: volna(true),
        },
        ONE_DARK => Themes::uniform(Theme::one_dark()),
        _ => {
            let builtin = BUILTIN.iter().find(|b| b.id == id)?;
            let palette = HostPalette::from_json(builtin.json).expect("bundled palettes are valid");
            Themes::uniform(Theme::from_host(&palette))
        }
    })
}

impl Theme {
    /// The built-in theme with this id, as a dark system shows it; the
    /// chrome (light) of Volna Mixed.
    pub fn builtin(id: &str) -> Option<Self> {
        resolve(id, true).map(|themes| themes.chrome)
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
        assert_eq!(choices().count(), BUILTIN.len() + CODED.len());
    }

    #[test]
    fn volna_follows_the_system_and_mixed_is_a_light_window_with_dark_data_panels() {
        let dark = |t: &Theme| t.appearance.is_dark();
        for system_dark in [true, false] {
            let volna = resolve(VOLNA, system_dark).unwrap();
            assert_eq!(dark(&volna.chrome), system_dark);
            assert_eq!(volna.canvas.editor.bg, volna.chrome.editor.bg);
            let mixed = resolve(VOLNA_MIXED, system_dark).unwrap();
            assert!(!dark(&mixed.chrome), "Mixed ignores the system");
            assert!(dark(&mixed.canvas));
            // The data panel is Volna Dark, unchanged: switching Mixed and Dark never changes the waves.
            let pinned = resolve(VOLNA_DARK, system_dark).unwrap();
            assert_eq!(mixed.canvas.wave_signal, pinned.canvas.wave_signal);
            assert_eq!(mixed.canvas.editor.bg, pinned.canvas.editor.bg);
            assert!(dark(&pinned.chrome));
            assert!(!dark(&resolve(VOLNA_LIGHT, system_dark).unwrap().chrome));
        }
        // Every built-in id resolves, and only those do.
        for (id, _) in choices() {
            assert!(resolve(id, false).is_some(), "{id}");
        }
        assert!(resolve("volna-sepia", true).is_none());
    }
}
