import React from 'react';
import { Icon } from '../core/Icon.jsx';
export function FeatureSection({ id, kicker, title, sub, center, features, children }) {
  return <section className={'v-section' + (center ? ' v-section--center' : '')} id={id}>
    <div className="v-wrap">
      {kicker && <p className="v-kicker">{kicker}</p>}
      {title && <h2 className="v-section__title">{title}</h2>}
      {sub && <p className="v-section__sub">{sub}</p>}
      {features && <div className="v-feature-grid" style={center ? { textAlign: 'left' } : undefined}>
        {features.map((f) => <article className="v-feature" key={f.title}>
          {f.icon && <div className="v-feature__icon"><Icon name={f.icon} size={18} /></div>}
          <h3>{f.title}</h3><p>{f.body}</p>{f.look && <p className="v-feature__look">{f.look}</p>}
        </article>)}
      </div>}
      {children}
    </div>
  </section>;
}
