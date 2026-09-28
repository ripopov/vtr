import React from 'react';
import { Icon } from '../core/Icon.jsx';
export function TopNav({ links = [], actions, logoSrc, brand = 'Volna', brandHref = '#', onNavigate }) {
  const click = (l) => (e) => { if (onNavigate) { e.preventDefault(); onNavigate(l); } };
  return <header className="v-nav">
    <div className="v-wrap v-nav__bar">
      <a className="v-brand" href={brandHref} onClick={onNavigate ? (e) => { e.preventDefault(); onNavigate({ href: brandHref, home: true }); } : undefined}>
        {logoSrc && <img src={logoSrc} alt="" />}{brand}
      </a>
      <nav className="v-nav__links" aria-label="Main">
        {links.map((l) => <a key={l.href} href={l.href} aria-current={l.current ? 'page' : undefined} onClick={click(l)}>{l.label}</a>)}
      </nav>
      <div className="v-nav__end">
        {actions}
        <details className="v-nav__menu">
          <summary className="v-btn v-btn--ghost v-btn--sm v-btn--icon" aria-label="Menu"><Icon name="menu" /></summary>
          <nav className="v-nav__sheet" aria-label="Main">
            {links.map((l) => <a key={l.href} href={l.href} aria-current={l.current ? 'page' : undefined} onClick={click(l)}>{l.label}</a>)}
          </nav>
        </details>
      </div>
    </div>
  </header>;
}
