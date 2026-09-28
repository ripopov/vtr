import React from 'react';
export function Chip({ tone = 'default', swatch, children }) {
  return <span className={'v-chip' + (tone !== 'default' ? ' v-chip--' + tone : '')}>{swatch && <span className="v-chip__swatch" style={{ '--swatch': swatch }}></span>}{children}</span>;
}
