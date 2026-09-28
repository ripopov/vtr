import React from 'react';
export function DocsSidebar({ groups = [], onNavigate, label = 'Contents' }) {
  return <details open>
    <summary>{label}<span aria-hidden="true">▾</span></summary>
    {groups.map((g) => <div className="v-docs__group" key={g.title}>
      <h4>{g.title}</h4>
      <ul>{g.items.map((it) => <li key={it.label}><a href={it.href} aria-current={it.current ? 'page' : undefined} onClick={onNavigate ? (e) => { e.preventDefault(); onNavigate(it); } : undefined}>{it.label}</a></li>)}</ul>
    </div>)}
  </details>;
}
