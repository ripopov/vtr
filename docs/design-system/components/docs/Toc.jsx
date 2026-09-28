import React from 'react';
export function Toc({ items = [], active, title = 'On this page' }) {
  return <nav className="v-toc" aria-label={title}>
    <h4>{title}</h4>
    <ul>{items.map((it) => <li key={it.id} className={it.level === 3 ? 'is-sub' : undefined}><a href={'#' + it.id} aria-current={active === it.id ? 'true' : undefined}>{it.label}</a></li>)}</ul>
  </nav>;
}
