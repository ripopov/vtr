import React from 'react';
export function Tabs({ tabs = [], value, onChange, variant = 'segmented', center, label = 'Tabs', idPrefix = 'tab' }) {
  const [own, setOwn] = React.useState(tabs[0] && tabs[0].id);
  const cur = value ?? own;
  const set = (id) => { onChange ? onChange(id) : setOwn(id); };
  const refs = React.useRef({});
  const onKey = (e, i) => {
    const d = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0; if (!d) return;
    const n = tabs[(i + d + tabs.length) % tabs.length]; set(n.id); refs.current[n.id] && refs.current[n.id].focus();
  };
  const active = tabs.find((t) => t.id === cur);
  return <div className="v-tabs">
    <div role="tablist" aria-label={label} className={['v-tabs__list', variant === 'line' && 'v-tabs__list--line', center && 'v-tabs__list--center'].filter(Boolean).join(' ')}>
      {tabs.map((t, i) => <button key={t.id} ref={(el) => (refs.current[t.id] = el)} role="tab" id={idPrefix + '-' + t.id} aria-controls={idPrefix + '-panel'} aria-selected={t.id === cur} tabIndex={t.id === cur ? 0 : -1} className="v-tabs__tab" onClick={() => set(t.id)} onKeyDown={(e) => onKey(e, i)}>{t.label}</button>)}
    </div>
    {active && active.content !== undefined && <div role="tabpanel" id={idPrefix + '-panel'} aria-labelledby={idPrefix + '-' + cur} className="v-tabs__panel">{active.content}</div>}
  </div>;
}
