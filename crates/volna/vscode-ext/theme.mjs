// This is the only VS Code-to-Volna colour mapping. Rust owns palette derivation.
export const tokens = {
  editor: { background: 'editor.background', foreground: 'editor.foreground' },
  panel: { background: 'sideBar.background', foreground: 'sideBar.foreground' },
  bar: { background: 'statusBar.background', foreground: 'statusBar.foreground' },
  elevated: { background: 'menu.background', foreground: 'menu.foreground' },
  tooltip: { background: 'editorHoverWidget.background', foreground: 'editorHoverWidget.foreground' },
  input: { background: 'input.background', foreground: 'input.foreground' },
  selection: { background: 'list.activeSelectionBackground', foreground: 'list.activeSelectionForeground' },
  hover: { background: 'list.hoverBackground', foreground: 'list.hoverForeground' },
  button: { background: 'button.background', foreground: 'button.foreground' },
  menu_hover: { background: 'menu.selectionBackground', foreground: 'menu.selectionForeground' },
  button_hover: 'button.hoverBackground', border: 'panel.border', panel_border: 'sideBar.border',
  input_border: 'input.border', focus: 'focusBorder', contrast_border: 'contrastBorder',
  muted: 'descriptionForeground', placeholder: 'input.placeholderForeground', icon: 'icon.foreground',
  accent: 'textLink.foreground', error: 'errorForeground',
  scrollbar: 'scrollbarSlider.background', scrollbar_hover: 'scrollbarSlider.hoverBackground',
  charts: { green: 'charts.green', red: 'charts.red', yellow: 'charts.yellow', blue: 'charts.blue', orange: 'charts.orange', purple: 'charts.purple' },
  cursor: 'editorCursor.foreground',
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

/** @typedef {'Light' | 'Dark' | 'HighContrastDark' | 'HighContrastLight'} Appearance */
export function snapshot(document, getStyle = getComputedStyle, fallback = false) {
  const classes = document.body.classList;
  // HC light also carries the legacy HC class. Prefer authoritative metadata.
  const kind = document.body.dataset?.vscodeThemeKind;
  const kinds = { 'vscode-light': 'Light', 'vscode-dark': 'Dark',
    'vscode-high-contrast-light': 'HighContrastLight', 'vscode-high-contrast': 'HighContrastDark' };
  let appearance = kinds[kind] ?? kinds[Object.keys(kinds).find(k => classes.contains(k))];
  const style = getStyle(document.body);
  const read = token => rgba(style.getPropertyValue(`--vscode-${token.replaceAll('.', '-')}`));
  const colors = Object.fromEntries(Object.entries(tokens).map(([name, token]) => [name,
    typeof token === 'string' ? read(token) : Object.fromEntries(Object.entries(token).map(([k, v]) => [k, read(v)]))]));
  if (!appearance) {
    if (!fallback) return undefined;
    // Keep delivered colours even without class metadata. Use the editor background
    // to pick fallback colours, then the OS preference only if no colour arrived.
    const bg = colors.editor.background;
    const dark = (bg === undefined || (bg & 255) === 0) ? (document.defaultView?.matchMedia?.('(prefers-color-scheme: dark)').matches ?? true)
      : (0.2126 * (bg >>> 24) + 0.7152 * ((bg >>> 16) & 255) + 0.0722 * ((bg >>> 8) & 255)) < 128;
    appearance = dark ? 'Dark' : 'Light';
  }
  return { appearance, colors };
}

// Observe before sampling. A bounded wait accommodates delayed host metadata;
// late arrival still updates the running viewer after fallback startup.
export function followTheme(setTheme, document = globalThis.document, Observer = MutationObserver, getStyle = getComputedStyle) {
  return new Promise(resolve => {
    let previous;
    let fallback = false;
    const update = () => {
      const theme = snapshot(document, getStyle, fallback);
      if (!theme) return;
      const serialized = JSON.stringify(theme);
      if (serialized !== previous) {
        setTheme(theme.appearance, theme.colors);
        previous = serialized;
      }
      fallback = true;
      clearTimeout(timer);
      resolve();
    };
    const timer = setTimeout(() => { fallback = true; update(); }, 250);
    const observer = new Observer(update);
    for (const element of [document.documentElement, document.body]) {
      observer.observe(element, { attributes: true, attributeFilter: ['style', 'class', 'data-vscode-theme-kind', 'data-vscode-theme-id'] });
    }
    document.defaultView?.addEventListener('pagehide', () => { clearTimeout(timer); observer.disconnect(); }, { once: true });
    update();
  });
}
