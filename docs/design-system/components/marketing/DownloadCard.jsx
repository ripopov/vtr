import React from 'react';
import { Icon } from '../core/Icon.jsx';
import { Button } from '../actions/Button.jsx';
export function DownloadCard({ platform, icon, badge, description, files = [], current, note }) {
  return <article className={'v-download' + (current ? ' v-download--current' : '')}>
    <div className="v-download__head">{icon && <Icon name={icon} size={20} />}<h3>{platform}</h3>{badge && <span className={'v-chip' + (current ? ' v-chip--accent' : '')}>{badge}</span>}</div>
    {description && <p className="v-download__desc">{description}</p>}
    <ul className="v-download__files">{files.map((f, i) => <li key={f.label} className="v-download__file">
      <Button href={f.href} variant={current && i === 0 ? 'primary' : 'secondary'} icon="download" meta={f.size}>{f.label}</Button>
      {f.meta && <div className="v-download__meta" style={{ flexBasis: '100%' }}>{f.meta}</div>}
    </li>)}</ul>
    {note && <div className="v-download__meta">{note}</div>}
  </article>;
}
