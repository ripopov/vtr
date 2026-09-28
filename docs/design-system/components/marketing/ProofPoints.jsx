import React from 'react';
export function ProofPoints({ items = [] }) {
  return <div className="v-stats">
    {items.map((s, i) => <div className="v-stat" key={i}>
      <div className="v-stat__value">{s.value}{s.unit && <small>{s.unit}</small>}</div>
      <div className="v-stat__label">{s.label}</div>
      {s.detail && <div className="v-stat__detail">{s.detail}</div>}
    </div>)}
  </div>;
}
