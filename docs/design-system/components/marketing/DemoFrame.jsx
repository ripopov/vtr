import React from 'react';
export function DemoFrame({ src, image, alt = '', title = 'Volna', path, actions = [], caption, state, aspect = '16/10', missing }) {
  const [loaded, setLoaded] = React.useState(false);
  const st = state || (src ? (loaded ? 'ready' : 'loading') : image ? 'ready' : 'missing');
  return <figure className="v-frame-fig" style={{ margin: 0 }}>
    <div className="v-frame">
      <div className="v-frame__bar"><b>{title}</b>{path && <span>{path}</span>}
        {actions.length > 0 && <span className="v-frame__actions">{actions.map((a) => <a key={a.label} href={a.href}>{a.label}</a>)}</span>}
      </div>
      <div className="v-frame__stage" style={{ aspectRatio: aspect }}>
        {src && <iframe src={src} title={title} onLoad={() => setLoaded(true)} allow="clipboard-read; clipboard-write" loading="lazy"></iframe>}
        {!src && image && <img src={image} alt={alt} loading="lazy" />}
        {st === 'loading' && <div className="v-frame__overlay" role="status"><div><div className="v-frame__spin"></div>Starting Volna in WebAssembly…</div></div>}
        {st === 'missing' && <div className="v-frame__overlay">{missing || <div>The live viewer is not built here.</div>}</div>}
      </div>
    </div>
    {caption && <figcaption className="v-frame__caption">{caption}</figcaption>}
  </figure>;
}
