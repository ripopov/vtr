// This is the only VS Code-to-Volna colour mapping. Rust owns palette derivation.
export const tokens = {
  bg_editor: 'editor.background', text: 'editor.foreground',
  bg_input: 'input.background', input_text: 'input.foreground', input_border: 'input.border',
  bg_panel: 'sideBar.background', panel_text: 'sideBar.foreground',
  bg_bar: 'statusBar.background', bar_text: 'statusBar.foreground',
  bg_elevated: 'editorWidget.background', elevated_text: 'editorWidget.foreground',
  border: 'panel.border', border_variant: 'sideBar.border',
  border_focused: 'focusBorder', contrast_border: 'contrastBorder',
  element_hover: 'list.hoverBackground', element_active: 'toolbar.activeBackground',
  element_selected: 'list.activeSelectionBackground', selected_text: 'list.activeSelectionForeground',
  text_muted: 'descriptionForeground', text_placeholder: 'input.placeholderForeground',
  text_accent: 'textLink.foreground', accent: 'focusBorder', icon: 'icon.foreground',
  button_bg: 'button.background', button_text: 'button.foreground', button_hover: 'button.hoverBackground',
  error: 'errorForeground', warning: 'editorWarning.foreground', success: 'charts.green',
  scrollbar_thumb: 'scrollbarSlider.background', scrollbar_thumb_hover: 'scrollbarSlider.hoverBackground',
  wave_signal: 'charts.green', wave_undef: 'charts.red', wave_highimp: 'charts.yellow',
  wave_dontcare: 'charts.blue', wave_weak: 'descriptionForeground', wave_cursor: 'editorCursor.foreground',
  marker_orange: 'charts.orange', marker_purple: 'charts.purple', marker_blue: 'charts.blue',
};

// VS Code serializes opaque colours as hex and translucent colours as rgba().
// Reject missing/malformed values so the Rust fallback remains authoritative.
export function rgba(value) {
  value = value.trim();
  if (/^#(?:[\da-f]{3}|[\da-f]{4}|[\da-f]{6}|[\da-f]{8})$/i.test(value)) {
    let hex = value.slice(1);
    if (hex.length < 5) hex = [...hex].map(c => c + c).join('');
    if (hex.length === 6) hex += 'ff';
    return Number.parseInt(hex, 16);
  }
  const match = /^(rgb|rgba)\(\s*([\d.]+)\s*,\s*([\d.]+)\s*,\s*([\d.]+)\s*(?:,\s*([\d.]+)\s*)?\)$/i.exec(value);
  if (!match || (match[1].toLowerCase() === 'rgba') !== (match[5] !== undefined)) return undefined;
  const channels = match.slice(2, 5).map(Number);
  const alpha = match[5] === undefined ? 1 : Number(match[5]);
  if (!channels.every(c => Number.isFinite(c) && c >= 0 && c <= 255) || !Number.isFinite(alpha) || alpha < 0 || alpha > 1) return undefined;
  return channels.reduce((packed, c) => packed * 256 + Math.round(c), 0) * 256 + Math.round(alpha * 255);
}

export function snapshot(document, getStyle = getComputedStyle) {
  const classes = document.body.classList;
  // HC light also carries the legacy high-contrast class. Check it first.
  const light = classes.contains('vscode-high-contrast-light') || classes.contains('vscode-light');
  const hc = classes.contains('vscode-high-contrast') || classes.contains('vscode-high-contrast-light');
  if (!light && !hc && !classes.contains('vscode-dark')) return undefined;
  const style = getStyle(document.body);
  const colors = {};
  for (const [name, token] of Object.entries(tokens)) {
    const value = rgba(style.getPropertyValue(`--vscode-${token.replaceAll('.', '-')}`));
    if (value !== undefined) colors[name] = value;
  }
  return { dark: !light, hc, colors };
}

// Observe before sampling, and sample again after wasm initialization. VS Code
// updates root styles and body classes in one task; MutationObserver batches them.
// Watching styles also catches customizations and switches within the same kind.
export function followTheme(setTheme, document = globalThis.document, Observer = MutationObserver, getStyle = getComputedStyle) {
  return new Promise(resolve => {
    let previous;
    const update = () => {
      const theme = snapshot(document, getStyle);
      if (!theme) return; // Wait for host theme metadata, never start with One Dark.
      const serialized = JSON.stringify(theme);
      if (serialized !== previous) {
        setTheme(theme.dark, theme.hc, theme.colors);
        previous = serialized;
      }
      resolve();
    };
    const observer = new Observer(update);
    for (const element of [document.documentElement, document.body]) {
      observer.observe(element, { attributes: true, attributeFilter: ['style', 'class', 'data-vscode-theme-kind', 'data-vscode-theme-id'] });
    }
    document.defaultView?.addEventListener('pagehide', () => observer.disconnect(), { once: true });
    update();
  });
}
