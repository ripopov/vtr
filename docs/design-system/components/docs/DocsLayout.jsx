import React from 'react';
export function DocsLayout({ sidebar, toc, crumbs = [], pager, children }) {
  return <div className="v-docs">
    <aside className="v-docs__side" aria-label="Documentation">{sidebar}</aside>
    <main className="v-docs__main" id="content">
      {crumbs.length > 0 && <nav className="v-docs__crumbs" aria-label="Breadcrumb">{crumbs.map((c, i) => <React.Fragment key={i}>{i > 0 && <span aria-hidden="true">/</span>}{c.href ? <a href={c.href}>{c.label}</a> : <span>{c.label}</span>}</React.Fragment>)}</nav>}
      <article className="v-prose">{children}</article>
      {pager && <nav className="v-docs__pager" aria-label="Pager">
        {pager.prev ? <a href={pager.prev.href}><small>← Previous</small>{pager.prev.label}</a> : <span></span>}
        {pager.next && <a href={pager.next.href}><small>Next →</small>{pager.next.label}</a>}
      </nav>}
    </main>
    {toc}
  </div>;
}
