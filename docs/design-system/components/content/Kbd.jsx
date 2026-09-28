import React from 'react';
export function Kbd({ keys, children }) {
  if (!keys) return <kbd className="v-kbd">{children}</kbd>;
  return <span className="v-keys">{keys.map((k, i) => <React.Fragment key={i}>{i > 0 && <span aria-hidden="true">+</span>}<kbd className="v-kbd">{k}</kbd></React.Fragment>)}</span>;
}
