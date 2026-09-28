import React from 'react';
import { Icon } from '../core/Icon.jsx';
function shLine(line, i) {
  if (/^\s*#/.test(line)) return <span key={i} className="tok-c">{line}</span>;
  const m = line.match(/^(\$ )(.*)$/);
  if (m) return <span key={i}><span className="tok-p">$ </span>{m[2].split(/(\s--?[\w-]+)/).map((p, j) => /^\s--?/.test(p) ? <span key={j} className="tok-k">{p}</span> : p)}</span>;
  return <span key={i} className="tok-c" style={{ color: 'var(--text-2)' }}>{line}</span>;
}
export function CodeBlock({ code = '', lang, filename, copy = true, bare, children }) {
  const [copied, setCopied] = React.useState(false);
  const text = code.replace(/\n$/, '');
  const doCopy = () => {
    const plain = lang === 'sh' ? text.split('\n').filter((l) => l.startsWith('$ ')).map((l) => l.slice(2)).join('\n') || text : text;
    navigator.clipboard && navigator.clipboard.writeText(plain);
    setCopied(true); setTimeout(() => setCopied(false), 1600);
  };
  const body = children || (lang === 'sh' ? text.split('\n').map((l, i) => <React.Fragment key={i}>{shLine(l, i)}{'\n'}</React.Fragment>) : text);
  return <div className={'v-code' + (bare ? ' v-code--bare' : '')}>
    <div className="v-code__head"><span>{filename || lang}</span>
      {copy && <button type="button" className="v-code__copy" onClick={doCopy} data-copied={copied}><Icon name={copied ? 'check' : 'copy'} size={13} />{copied ? 'Copied' : 'Copy'}</button>}
      <span className="v-visually-hidden" aria-live="polite">{copied ? 'Copied to clipboard' : ''}</span>
    </div>
    <pre><code>{body}</code></pre>
  </div>;
}
