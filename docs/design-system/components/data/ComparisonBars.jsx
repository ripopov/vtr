import React from 'react';
export function ComparisonBars({ title, rows = [], note }) {
  return <div className="v-bars">
    {title && <h3 className="v-bars__title">{title}</h3>}
    {rows.map((r, i) => {
      const share = Math.max(0, Math.min(1, r.ours.value / r.theirs.value));
      const pct = (share * 100).toFixed(2) + '%';
      return <div className="v-bar" key={i} role="img" aria-label={r.theirs.name + ' ' + r.theirs.display + ', ' + r.ours.name + ' ' + r.ours.display + ': ' + r.ratio + ' ' + r.label}>
        <div className="v-bar__ratio"><b>{r.ratio}</b><span>{r.label}</span></div>
        <div className="v-bar__track"><div className="v-bar__fill v-bar__fill--theirs" style={{ width: '100%' }}>{r.theirs.name} {r.theirs.display}</div></div>
        <div className="v-bar__track"><div className="v-bar__fill v-bar__fill--ours" style={{ width: pct }}>{r.ours.name} {r.ours.display}</div><div className="v-bar__ghost" style={{ left: pct }}></div></div>
      </div>;
    })}
    {note && <p className="v-bars__note">{note}</p>}
  </div>;
}
