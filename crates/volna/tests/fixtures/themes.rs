pub fn real_palettes() -> [HostPalette; 4] {
    [
        include_str!("vscode/dark-modern.txt"),
        include_str!("vscode/light-modern.txt"),
        include_str!("vscode/dark-high-contrast.txt"),
        include_str!("vscode/light-high-contrast.txt"),
    ].map(vscode::host_palette)
}
