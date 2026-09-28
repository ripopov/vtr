import React from 'react';
export function Footer({ about, columns = [], base }) {
  return <footer className="v-footer">
    <div className="v-wrap">
      <div className="v-footer__grid">
        <div className="v-footer__about">{about}</div>
        {columns.map((c) => <div key={c.title}><h4>{c.title}</h4><ul>{c.links.map((l) => <li key={l.label}><a href={l.href}>{l.label}</a></li>)}</ul></div>)}
      </div>
      {base && <div className="v-footer__base">{base}</div>}
    </div>
  </footer>;
}
