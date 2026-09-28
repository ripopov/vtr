import React from 'react';
export function BenchTable({ columns = [], rows = [], caption, label }) {
  return <div className="v-table-wrap" role="region" aria-label={label || caption} tabIndex={0}>
    <table className="v-table">
      <thead><tr>{columns.map((c) => <th key={c.key} scope="col" className={c.ours ? 'is-ours' : undefined}>{c.label}</th>)}</tr></thead>
      <tbody>{rows.map((r, i) => <tr key={i}>{columns.map((c, j) => {
        const v = r[c.key];
        const cls = [c.ours && 'is-ours', c.ratio && 'is-ratio'].filter(Boolean).join(' ') || undefined;
        return j === 0 ? <th key={c.key} scope="row" style={{ fontWeight: 400, fontFamily: 'var(--font-sans)', color: 'var(--text-1)', textAlign: 'left' }}>{v}</th> : <td key={c.key} className={cls}>{c.ours ? <b style={{ fontWeight: 400 }}>{v}</b> : v}</td>;
      })}</tr>)}</tbody>
      {caption && <caption>{caption}</caption>}
    </table>
  </div>;
}
