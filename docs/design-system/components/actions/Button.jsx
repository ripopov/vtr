import React from 'react';
import { Icon } from '../core/Icon.jsx';
export function Button({ variant = 'secondary', size = 'md', href, icon, iconRight, meta, children, disabled, className = '', ...rest }) {
  const cls = ['v-btn', variant !== 'secondary' && 'v-btn--' + variant, size !== 'md' && 'v-btn--' + size, !children && icon && 'v-btn--icon', className].filter(Boolean).join(' ');
  const inner = <>{icon && <Icon name={icon} size={size === 'sm' ? 14 : 16} />}{children}{meta && <span className="v-btn__meta">{meta}</span>}{iconRight && <Icon name={iconRight} size={size === 'sm' ? 14 : 16} />}</>;
  if (href) return <a className={cls} href={disabled ? undefined : href} aria-disabled={disabled || undefined} {...rest}>{inner}</a>;
  return <button type="button" className={cls} disabled={disabled} {...rest}>{inner}</button>;
}
