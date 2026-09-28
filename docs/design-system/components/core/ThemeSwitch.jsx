import React from 'react';
function apply(mode) {
  const root = document.documentElement;
  const dark = mode === 'dark' || (mode === 'system' && matchMedia('(prefers-color-scheme: dark)').matches);
  root.dataset.theme = dark ? 'dark' : 'light';
  try { localStorage.setItem('volna-theme', mode); } catch (e) {}
}
export function ThemeSwitch({ value, onChange, label = 'Theme' }) {
  const [own, setOwn] = React.useState(() => { try { return localStorage.getItem('volna-theme') || 'system'; } catch (e) { return 'system'; } });
  const mode = value ?? own;
  const pick = (m) => { if (onChange) onChange(m); else { setOwn(m); apply(m); } };
  const opts = [['light', 'sun', 'Light'], ['dark', 'moon', 'Dark'], ['system', 'monitor', 'System']];
  return <div className="v-seg" role="group" aria-label={label}>
    {opts.map(([m, icon, name]) => <button key={m} type="button" aria-pressed={mode === m} title={name} aria-label={name} onClick={() => pick(m)}>
      <span className="v-icon" aria-hidden="true" style={{ '--icon': 'url(https://unpkg.com/lucide-static@0.469.0/icons/' + icon + '.svg)', '--icon-size': '14px' }}></span>
    </button>)}
  </div>;
}
