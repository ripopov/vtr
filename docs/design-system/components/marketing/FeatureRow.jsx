import React from 'react';
export function FeatureRow({ title, kicker, body, bullets, media, flip }) {
  return <div className={'v-feature-row' + (flip ? ' v-feature-row--flip' : '')}>
    <div className="v-feature-row__text">
      {kicker && <p className="v-kicker">{kicker}</p>}
      <h3>{title}</h3>{body && <p>{body}</p>}
      {bullets && <ul>{bullets.map((b, i) => <li key={i}><span>{b}</span></li>)}</ul>}
    </div>
    <div>{media}</div>
  </div>;
}
