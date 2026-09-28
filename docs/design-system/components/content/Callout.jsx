import React from 'react';
import { Icon } from '../core/Icon.jsx';
const ICONS = { info: 'info', success: 'circle-check', warning: 'triangle-alert', danger: 'octagon-alert' };
export function Callout({ tone = 'info', title, icon, children }) {
  return <aside className={'v-callout' + (tone !== 'info' ? ' v-callout--' + tone : '')}>
    <Icon name={icon || ICONS[tone]} />
    <div className="v-callout__title">{title}</div>
    {children && <div className="v-callout__body">{children}</div>}
  </aside>;
}
