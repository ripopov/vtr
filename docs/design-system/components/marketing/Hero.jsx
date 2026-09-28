import React from 'react';
export function Hero({ pill, title, lede, actions, fine, align = 'center', children }) {
  return <section className={'v-hero' + (align === 'left' ? ' v-hero--left' : '')}>
    <div className="v-wrap">
      {pill && <a className="v-pill" href={pill.href || '#'}><b>{pill.tag}</b><span className="v-pill__label">{pill.label}</span></a>}
      <h1 className="v-hero__title">{title}</h1>
      {lede && <p className="v-hero__lede">{lede}</p>}
      {actions && <div className="v-hero__cta">{actions}</div>}
      {fine && <p className="v-hero__fine">{fine}</p>}
    </div>
    {children}
  </section>;
}
