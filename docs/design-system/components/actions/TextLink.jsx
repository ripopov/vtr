import React from 'react';
export function TextLink({ href, variant = 'default', arrow, external, children, ...rest }) {
  const cls = ['v-link', variant === 'quiet' && 'v-link--quiet', arrow && 'v-link--arrow', external && 'v-link--external'].filter(Boolean).join(' ');
  return <a className={cls} href={href} {...(external ? { target: '_blank', rel: 'noopener' } : {})} {...rest}>{children}</a>;
}
