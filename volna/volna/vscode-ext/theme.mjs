// Transport only. Rust owns all colour parsing, appearance inference and mapping.
export function snapshot(document = globalThis.document, getStyle = getComputedStyle) {
  const body = document.body;
  const kind = body.dataset.vscodeThemeKind || ['vscode-high-contrast-light', 'vscode-high-contrast'].find(c => body.classList.contains(c)) || [...body.classList].find(c =>
    c === 'vscode-light' || c === 'vscode-dark') || '';
  const style = getStyle(body);
  const lines = [...style].filter(name => name.startsWith('--vscode-')).sort()
    .map(name => `${name}=${style.getPropertyValue(name).trim()}`);
  return [`kind=${kind}`, ...lines].join('\n');
}

// Observe before WASM starts. Its startup callback reads current() synchronously;
// start() flushes any changes during initialization, without a startup gate.
export function watchTheme(setTheme, document = globalThis.document, Observer = MutationObserver, getStyle = getComputedStyle) {
  let previous, ready = false;
  const current = () => (previous = snapshot(document, getStyle));
  const update = () => {
    if (!ready) return;
    const next = snapshot(document, getStyle);
    if (next !== previous) {
      setTheme(next);
      previous = next;
    }
  };
  const observer = new Observer(update);
  for (const element of [document.documentElement, document.body]) {
    observer.observe(element, { attributes: true, attributeFilter: ['style', 'class', 'data-vscode-theme-kind', 'data-vscode-theme-id'] });
  }
  const dispose = () => { ready = false; observer.disconnect(); };
  document.defaultView?.addEventListener('pagehide', dispose, { once: true });
  return { current, start() { ready = true; update(); }, dispose };
}
