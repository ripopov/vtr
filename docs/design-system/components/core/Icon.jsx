import React from 'react';
const BASE = 'https://unpkg.com/lucide-static@0.469.0/icons/';
export function Icon({ name, size = 16, label, className = '', style }) {
  return <span className={'v-icon ' + className} role={label ? 'img' : undefined} aria-label={label} aria-hidden={label ? undefined : true}
    style={{ '--icon': 'url(' + BASE + name + '.svg)', '--icon-size': size + 'px', ...style }}></span>;
}
