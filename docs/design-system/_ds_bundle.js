/* @ds-bundle: {"format":4,"namespace":"VolnaDesignSystem_3096af","components":[{"name":"Button","sourcePath":"components/actions/Button.jsx"},{"name":"TextLink","sourcePath":"components/actions/TextLink.jsx"},{"name":"Callout","sourcePath":"components/content/Callout.jsx"},{"name":"Chip","sourcePath":"components/content/Chip.jsx"},{"name":"CodeBlock","sourcePath":"components/content/CodeBlock.jsx"},{"name":"Kbd","sourcePath":"components/content/Kbd.jsx"},{"name":"Icon","sourcePath":"components/core/Icon.jsx"},{"name":"ThemeSwitch","sourcePath":"components/core/ThemeSwitch.jsx"},{"name":"BenchTable","sourcePath":"components/data/BenchTable.jsx"},{"name":"ComparisonBars","sourcePath":"components/data/ComparisonBars.jsx"},{"name":"DocsLayout","sourcePath":"components/docs/DocsLayout.jsx"},{"name":"DocsSidebar","sourcePath":"components/docs/DocsSidebar.jsx"},{"name":"Toc","sourcePath":"components/docs/Toc.jsx"},{"name":"DemoFrame","sourcePath":"components/marketing/DemoFrame.jsx"},{"name":"DownloadCard","sourcePath":"components/marketing/DownloadCard.jsx"},{"name":"FeatureRow","sourcePath":"components/marketing/FeatureRow.jsx"},{"name":"FeatureSection","sourcePath":"components/marketing/FeatureSection.jsx"},{"name":"Hero","sourcePath":"components/marketing/Hero.jsx"},{"name":"ProofPoints","sourcePath":"components/marketing/ProofPoints.jsx"},{"name":"Footer","sourcePath":"components/navigation/Footer.jsx"},{"name":"Tabs","sourcePath":"components/navigation/Tabs.jsx"},{"name":"TopNav","sourcePath":"components/navigation/TopNav.jsx"}],"sourceHashes":{"components/actions/Button.jsx":"70982395da9f","components/actions/TextLink.jsx":"ecb58807717f","components/content/Callout.jsx":"0c5d8c441f49","components/content/Chip.jsx":"0bcea7cf62b8","components/content/CodeBlock.jsx":"1dd300013f35","components/content/Kbd.jsx":"8eade4d1db5f","components/core/Icon.jsx":"b9d48a2cf67f","components/core/ThemeSwitch.jsx":"a2cbee5bb8b4","components/data/BenchTable.jsx":"4d0f87d7b4d6","components/data/ComparisonBars.jsx":"a71d5f7b4fac","components/docs/DocsLayout.jsx":"ddb8edc9d0a5","components/docs/DocsSidebar.jsx":"e60acafec355","components/docs/Toc.jsx":"2e62bb37ffd1","components/marketing/DemoFrame.jsx":"6b56ee30aaf4","components/marketing/DownloadCard.jsx":"3990b3e28fd8","components/marketing/FeatureRow.jsx":"a49512880ffb","components/marketing/FeatureSection.jsx":"4496e44f92ba","components/marketing/Hero.jsx":"072ef2ab36dd","components/marketing/ProofPoints.jsx":"af49a92815fd","components/navigation/Footer.jsx":"c5065b8ce678","components/navigation/Tabs.jsx":"4b2e5d9acd3b","components/navigation/TopNav.jsx":"551a9ed84e8f"},"inlinedExternals":[],"unexposedExports":[]} */

(() => {

const __ds_ns = (window.VolnaDesignSystem_3096af = window.VolnaDesignSystem_3096af || {});

const __ds_scope = {};

(__ds_ns.__errors = __ds_ns.__errors || []);

// components/actions/TextLink.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
function TextLink({
  href,
  variant = 'default',
  arrow,
  external,
  children,
  ...rest
}) {
  const cls = ['v-link', variant === 'quiet' && 'v-link--quiet', arrow && 'v-link--arrow', external && 'v-link--external'].filter(Boolean).join(' ');
  return /*#__PURE__*/React.createElement("a", _extends({
    className: cls,
    href: href
  }, external ? {
    target: '_blank',
    rel: 'noopener'
  } : {}, rest), children);
}
Object.assign(__ds_scope, { TextLink });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/actions/TextLink.jsx", error: String((e && e.message) || e) }); }

// components/content/Chip.jsx
try { (() => {
function Chip({
  tone = 'default',
  swatch,
  children
}) {
  return /*#__PURE__*/React.createElement("span", {
    className: 'v-chip' + (tone !== 'default' ? ' v-chip--' + tone : '')
  }, swatch && /*#__PURE__*/React.createElement("span", {
    className: "v-chip__swatch",
    style: {
      '--swatch': swatch
    }
  }), children);
}
Object.assign(__ds_scope, { Chip });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/content/Chip.jsx", error: String((e && e.message) || e) }); }

// components/content/Kbd.jsx
try { (() => {
function Kbd({
  keys,
  children
}) {
  if (!keys) return /*#__PURE__*/React.createElement("kbd", {
    className: "v-kbd"
  }, children);
  return /*#__PURE__*/React.createElement("span", {
    className: "v-keys"
  }, keys.map((k, i) => /*#__PURE__*/React.createElement(React.Fragment, {
    key: i
  }, i > 0 && /*#__PURE__*/React.createElement("span", {
    "aria-hidden": "true"
  }, "+"), /*#__PURE__*/React.createElement("kbd", {
    className: "v-kbd"
  }, k))));
}
Object.assign(__ds_scope, { Kbd });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/content/Kbd.jsx", error: String((e && e.message) || e) }); }

// components/core/Icon.jsx
try { (() => {
const BASE = 'https://unpkg.com/lucide-static@0.469.0/icons/';
function Icon({
  name,
  size = 16,
  label,
  className = '',
  style
}) {
  return /*#__PURE__*/React.createElement("span", {
    className: 'v-icon ' + className,
    role: label ? 'img' : undefined,
    "aria-label": label,
    "aria-hidden": label ? undefined : true,
    style: {
      '--icon': 'url(' + BASE + name + '.svg)',
      '--icon-size': size + 'px',
      ...style
    }
  });
}
Object.assign(__ds_scope, { Icon });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/Icon.jsx", error: String((e && e.message) || e) }); }

// components/actions/Button.jsx
try { (() => {
function _extends() { return _extends = Object.assign ? Object.assign.bind() : function (n) { for (var e = 1; e < arguments.length; e++) { var t = arguments[e]; for (var r in t) ({}).hasOwnProperty.call(t, r) && (n[r] = t[r]); } return n; }, _extends.apply(null, arguments); }
function Button({
  variant = 'secondary',
  size = 'md',
  href,
  icon,
  iconRight,
  meta,
  children,
  disabled,
  className = '',
  ...rest
}) {
  const cls = ['v-btn', variant !== 'secondary' && 'v-btn--' + variant, size !== 'md' && 'v-btn--' + size, !children && icon && 'v-btn--icon', className].filter(Boolean).join(' ');
  const inner = /*#__PURE__*/React.createElement(React.Fragment, null, icon && /*#__PURE__*/React.createElement(__ds_scope.Icon, {
    name: icon,
    size: size === 'sm' ? 14 : 16
  }), children, meta && /*#__PURE__*/React.createElement("span", {
    className: "v-btn__meta"
  }, meta), iconRight && /*#__PURE__*/React.createElement(__ds_scope.Icon, {
    name: iconRight,
    size: size === 'sm' ? 14 : 16
  }));
  if (href) return /*#__PURE__*/React.createElement("a", _extends({
    className: cls,
    href: disabled ? undefined : href,
    "aria-disabled": disabled || undefined
  }, rest), inner);
  return /*#__PURE__*/React.createElement("button", _extends({
    type: "button",
    className: cls,
    disabled: disabled
  }, rest), inner);
}
Object.assign(__ds_scope, { Button });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/actions/Button.jsx", error: String((e && e.message) || e) }); }

// components/content/Callout.jsx
try { (() => {
const ICONS = {
  info: 'info',
  success: 'circle-check',
  warning: 'triangle-alert',
  danger: 'octagon-alert'
};
function Callout({
  tone = 'info',
  title,
  icon,
  children
}) {
  return /*#__PURE__*/React.createElement("aside", {
    className: 'v-callout' + (tone !== 'info' ? ' v-callout--' + tone : '')
  }, /*#__PURE__*/React.createElement(__ds_scope.Icon, {
    name: icon || ICONS[tone]
  }), /*#__PURE__*/React.createElement("div", {
    className: "v-callout__title"
  }, title), children && /*#__PURE__*/React.createElement("div", {
    className: "v-callout__body"
  }, children));
}
Object.assign(__ds_scope, { Callout });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/content/Callout.jsx", error: String((e && e.message) || e) }); }

// components/content/CodeBlock.jsx
try { (() => {
function shLine(line, i) {
  if (/^\s*#/.test(line)) return /*#__PURE__*/React.createElement("span", {
    key: i,
    className: "tok-c"
  }, line);
  const m = line.match(/^(\$ )(.*)$/);
  if (m) return /*#__PURE__*/React.createElement("span", {
    key: i
  }, /*#__PURE__*/React.createElement("span", {
    className: "tok-p"
  }, "$ "), m[2].split(/(\s--?[\w-]+)/).map((p, j) => /^\s--?/.test(p) ? /*#__PURE__*/React.createElement("span", {
    key: j,
    className: "tok-k"
  }, p) : p));
  return /*#__PURE__*/React.createElement("span", {
    key: i,
    className: "tok-c",
    style: {
      color: 'var(--text-2)'
    }
  }, line);
}
function CodeBlock({
  code = '',
  lang,
  filename,
  copy = true,
  bare,
  children
}) {
  const [copied, setCopied] = React.useState(false);
  const text = code.replace(/\n$/, '');
  const doCopy = () => {
    const plain = lang === 'sh' ? text.split('\n').filter(l => l.startsWith('$ ')).map(l => l.slice(2)).join('\n') || text : text;
    navigator.clipboard && navigator.clipboard.writeText(plain);
    setCopied(true);
    setTimeout(() => setCopied(false), 1600);
  };
  const body = children || (lang === 'sh' ? text.split('\n').map((l, i) => /*#__PURE__*/React.createElement(React.Fragment, {
    key: i
  }, shLine(l, i), '\n')) : text);
  return /*#__PURE__*/React.createElement("div", {
    className: 'v-code' + (bare ? ' v-code--bare' : '')
  }, /*#__PURE__*/React.createElement("div", {
    className: "v-code__head"
  }, /*#__PURE__*/React.createElement("span", null, filename || lang), copy && /*#__PURE__*/React.createElement("button", {
    type: "button",
    className: "v-code__copy",
    onClick: doCopy,
    "data-copied": copied
  }, /*#__PURE__*/React.createElement(__ds_scope.Icon, {
    name: copied ? 'check' : 'copy',
    size: 13
  }), copied ? 'Copied' : 'Copy'), /*#__PURE__*/React.createElement("span", {
    className: "v-visually-hidden",
    "aria-live": "polite"
  }, copied ? 'Copied to clipboard' : '')), /*#__PURE__*/React.createElement("pre", null, /*#__PURE__*/React.createElement("code", null, body)));
}
Object.assign(__ds_scope, { CodeBlock });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/content/CodeBlock.jsx", error: String((e && e.message) || e) }); }

// components/core/ThemeSwitch.jsx
try { (() => {
function apply(mode) {
  const root = document.documentElement;
  const dark = mode === 'dark' || mode === 'system' && matchMedia('(prefers-color-scheme: dark)').matches;
  root.dataset.theme = dark ? 'dark' : 'light';
  try {
    localStorage.setItem('volna-theme', mode);
  } catch (e) {}
}
function ThemeSwitch({
  value,
  onChange,
  label = 'Theme'
}) {
  const [own, setOwn] = React.useState(() => {
    try {
      return localStorage.getItem('volna-theme') || 'system';
    } catch (e) {
      return 'system';
    }
  });
  const mode = value ?? own;
  const pick = m => {
    if (onChange) onChange(m);else {
      setOwn(m);
      apply(m);
    }
  };
  const opts = [['light', 'sun', 'Light'], ['dark', 'moon', 'Dark'], ['system', 'monitor', 'System']];
  return /*#__PURE__*/React.createElement("div", {
    className: "v-seg",
    role: "group",
    "aria-label": label
  }, opts.map(([m, icon, name]) => /*#__PURE__*/React.createElement("button", {
    key: m,
    type: "button",
    "aria-pressed": mode === m,
    title: name,
    "aria-label": name,
    onClick: () => pick(m)
  }, /*#__PURE__*/React.createElement("span", {
    className: "v-icon",
    "aria-hidden": "true",
    style: {
      '--icon': 'url(https://unpkg.com/lucide-static@0.469.0/icons/' + icon + '.svg)',
      '--icon-size': '14px'
    }
  }))));
}
Object.assign(__ds_scope, { ThemeSwitch });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/core/ThemeSwitch.jsx", error: String((e && e.message) || e) }); }

// components/data/BenchTable.jsx
try { (() => {
function BenchTable({
  columns = [],
  rows = [],
  caption,
  label
}) {
  return /*#__PURE__*/React.createElement("div", {
    className: "v-table-wrap",
    role: "region",
    "aria-label": label || caption,
    tabIndex: 0
  }, /*#__PURE__*/React.createElement("table", {
    className: "v-table"
  }, /*#__PURE__*/React.createElement("thead", null, /*#__PURE__*/React.createElement("tr", null, columns.map(c => /*#__PURE__*/React.createElement("th", {
    key: c.key,
    scope: "col",
    className: c.ours ? 'is-ours' : undefined
  }, c.label)))), /*#__PURE__*/React.createElement("tbody", null, rows.map((r, i) => /*#__PURE__*/React.createElement("tr", {
    key: i
  }, columns.map((c, j) => {
    const v = r[c.key];
    const cls = [c.ours && 'is-ours', c.ratio && 'is-ratio'].filter(Boolean).join(' ') || undefined;
    return j === 0 ? /*#__PURE__*/React.createElement("th", {
      key: c.key,
      scope: "row",
      style: {
        fontWeight: 400,
        fontFamily: 'var(--font-sans)',
        color: 'var(--text-1)',
        textAlign: 'left'
      }
    }, v) : /*#__PURE__*/React.createElement("td", {
      key: c.key,
      className: cls
    }, c.ours ? /*#__PURE__*/React.createElement("b", {
      style: {
        fontWeight: 400
      }
    }, v) : v);
  })))), caption && /*#__PURE__*/React.createElement("caption", null, caption)));
}
Object.assign(__ds_scope, { BenchTable });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/data/BenchTable.jsx", error: String((e && e.message) || e) }); }

// components/data/ComparisonBars.jsx
try { (() => {
function ComparisonBars({
  title,
  rows = [],
  note
}) {
  return /*#__PURE__*/React.createElement("div", {
    className: "v-bars"
  }, title && /*#__PURE__*/React.createElement("h3", {
    className: "v-bars__title"
  }, title), rows.map((r, i) => {
    const share = Math.max(0, Math.min(1, r.ours.value / r.theirs.value));
    const pct = (share * 100).toFixed(2) + '%';
    return /*#__PURE__*/React.createElement("div", {
      className: "v-bar",
      key: i,
      role: "img",
      "aria-label": r.theirs.name + ' ' + r.theirs.display + ', ' + r.ours.name + ' ' + r.ours.display + ': ' + r.ratio + ' ' + r.label
    }, /*#__PURE__*/React.createElement("div", {
      className: "v-bar__ratio"
    }, /*#__PURE__*/React.createElement("b", null, r.ratio), /*#__PURE__*/React.createElement("span", null, r.label)), /*#__PURE__*/React.createElement("div", {
      className: "v-bar__track"
    }, /*#__PURE__*/React.createElement("div", {
      className: "v-bar__fill v-bar__fill--theirs",
      style: {
        width: '100%'
      }
    }, r.theirs.name, " ", r.theirs.display)), /*#__PURE__*/React.createElement("div", {
      className: "v-bar__track"
    }, /*#__PURE__*/React.createElement("div", {
      className: "v-bar__fill v-bar__fill--ours",
      style: {
        width: pct
      }
    }, r.ours.name, " ", r.ours.display), /*#__PURE__*/React.createElement("div", {
      className: "v-bar__ghost",
      style: {
        left: pct
      }
    })));
  }), note && /*#__PURE__*/React.createElement("p", {
    className: "v-bars__note"
  }, note));
}
Object.assign(__ds_scope, { ComparisonBars });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/data/ComparisonBars.jsx", error: String((e && e.message) || e) }); }

// components/docs/DocsLayout.jsx
try { (() => {
function DocsLayout({
  sidebar,
  toc,
  crumbs = [],
  pager,
  children
}) {
  return /*#__PURE__*/React.createElement("div", {
    className: "v-docs"
  }, /*#__PURE__*/React.createElement("aside", {
    className: "v-docs__side",
    "aria-label": "Documentation"
  }, sidebar), /*#__PURE__*/React.createElement("main", {
    className: "v-docs__main",
    id: "content"
  }, crumbs.length > 0 && /*#__PURE__*/React.createElement("nav", {
    className: "v-docs__crumbs",
    "aria-label": "Breadcrumb"
  }, crumbs.map((c, i) => /*#__PURE__*/React.createElement(React.Fragment, {
    key: i
  }, i > 0 && /*#__PURE__*/React.createElement("span", {
    "aria-hidden": "true"
  }, "/"), c.href ? /*#__PURE__*/React.createElement("a", {
    href: c.href
  }, c.label) : /*#__PURE__*/React.createElement("span", null, c.label)))), /*#__PURE__*/React.createElement("article", {
    className: "v-prose"
  }, children), pager && /*#__PURE__*/React.createElement("nav", {
    className: "v-docs__pager",
    "aria-label": "Pager"
  }, pager.prev ? /*#__PURE__*/React.createElement("a", {
    href: pager.prev.href
  }, /*#__PURE__*/React.createElement("small", null, "\u2190 Previous"), pager.prev.label) : /*#__PURE__*/React.createElement("span", null), pager.next && /*#__PURE__*/React.createElement("a", {
    href: pager.next.href
  }, /*#__PURE__*/React.createElement("small", null, "Next \u2192"), pager.next.label))), toc);
}
Object.assign(__ds_scope, { DocsLayout });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/docs/DocsLayout.jsx", error: String((e && e.message) || e) }); }

// components/docs/DocsSidebar.jsx
try { (() => {
function DocsSidebar({
  groups = [],
  onNavigate,
  label = 'Contents'
}) {
  return /*#__PURE__*/React.createElement("details", {
    open: true
  }, /*#__PURE__*/React.createElement("summary", null, label, /*#__PURE__*/React.createElement("span", {
    "aria-hidden": "true"
  }, "\u25BE")), groups.map(g => /*#__PURE__*/React.createElement("div", {
    className: "v-docs__group",
    key: g.title
  }, /*#__PURE__*/React.createElement("h4", null, g.title), /*#__PURE__*/React.createElement("ul", null, g.items.map(it => /*#__PURE__*/React.createElement("li", {
    key: it.label
  }, /*#__PURE__*/React.createElement("a", {
    href: it.href,
    "aria-current": it.current ? 'page' : undefined,
    onClick: onNavigate ? e => {
      e.preventDefault();
      onNavigate(it);
    } : undefined
  }, it.label)))))));
}
Object.assign(__ds_scope, { DocsSidebar });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/docs/DocsSidebar.jsx", error: String((e && e.message) || e) }); }

// components/docs/Toc.jsx
try { (() => {
function Toc({
  items = [],
  active,
  title = 'On this page'
}) {
  return /*#__PURE__*/React.createElement("nav", {
    className: "v-toc",
    "aria-label": title
  }, /*#__PURE__*/React.createElement("h4", null, title), /*#__PURE__*/React.createElement("ul", null, items.map(it => /*#__PURE__*/React.createElement("li", {
    key: it.id,
    className: it.level === 3 ? 'is-sub' : undefined
  }, /*#__PURE__*/React.createElement("a", {
    href: '#' + it.id,
    "aria-current": active === it.id ? 'true' : undefined
  }, it.label)))));
}
Object.assign(__ds_scope, { Toc });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/docs/Toc.jsx", error: String((e && e.message) || e) }); }

// components/marketing/DemoFrame.jsx
try { (() => {
function DemoFrame({
  src,
  image,
  alt = '',
  title = 'Volna',
  path,
  actions = [],
  caption,
  state,
  aspect = '16/10',
  missing
}) {
  const [loaded, setLoaded] = React.useState(false);
  const st = state || (src ? loaded ? 'ready' : 'loading' : image ? 'ready' : 'missing');
  return /*#__PURE__*/React.createElement("figure", {
    className: "v-frame-fig",
    style: {
      margin: 0
    }
  }, /*#__PURE__*/React.createElement("div", {
    className: "v-frame"
  }, /*#__PURE__*/React.createElement("div", {
    className: "v-frame__bar"
  }, /*#__PURE__*/React.createElement("b", null, title), path && /*#__PURE__*/React.createElement("span", null, path), actions.length > 0 && /*#__PURE__*/React.createElement("span", {
    className: "v-frame__actions"
  }, actions.map(a => /*#__PURE__*/React.createElement("a", {
    key: a.label,
    href: a.href
  }, a.label)))), /*#__PURE__*/React.createElement("div", {
    className: "v-frame__stage",
    style: {
      aspectRatio: aspect
    }
  }, src && /*#__PURE__*/React.createElement("iframe", {
    src: src,
    title: title,
    onLoad: () => setLoaded(true),
    allow: "clipboard-read; clipboard-write",
    loading: "lazy"
  }), !src && image && /*#__PURE__*/React.createElement("img", {
    src: image,
    alt: alt,
    loading: "lazy"
  }), st === 'loading' && /*#__PURE__*/React.createElement("div", {
    className: "v-frame__overlay",
    role: "status"
  }, /*#__PURE__*/React.createElement("div", null, /*#__PURE__*/React.createElement("div", {
    className: "v-frame__spin"
  }), "Starting Volna in WebAssembly\u2026")), st === 'missing' && /*#__PURE__*/React.createElement("div", {
    className: "v-frame__overlay"
  }, missing || /*#__PURE__*/React.createElement("div", null, "The live viewer is not built here.")))), caption && /*#__PURE__*/React.createElement("figcaption", {
    className: "v-frame__caption"
  }, caption));
}
Object.assign(__ds_scope, { DemoFrame });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/marketing/DemoFrame.jsx", error: String((e && e.message) || e) }); }

// components/marketing/DownloadCard.jsx
try { (() => {
function DownloadCard({
  platform,
  icon,
  badge,
  description,
  files = [],
  current,
  note
}) {
  return /*#__PURE__*/React.createElement("article", {
    className: 'v-download' + (current ? ' v-download--current' : '')
  }, /*#__PURE__*/React.createElement("div", {
    className: "v-download__head"
  }, icon && /*#__PURE__*/React.createElement(__ds_scope.Icon, {
    name: icon,
    size: 20
  }), /*#__PURE__*/React.createElement("h3", null, platform), badge && /*#__PURE__*/React.createElement("span", {
    className: 'v-chip' + (current ? ' v-chip--accent' : '')
  }, badge)), description && /*#__PURE__*/React.createElement("p", {
    className: "v-download__desc"
  }, description), /*#__PURE__*/React.createElement("ul", {
    className: "v-download__files"
  }, files.map((f, i) => /*#__PURE__*/React.createElement("li", {
    key: f.label,
    className: "v-download__file"
  }, /*#__PURE__*/React.createElement(__ds_scope.Button, {
    href: f.href,
    variant: current && i === 0 ? 'primary' : 'secondary',
    icon: "download",
    meta: f.size
  }, f.label), f.meta && /*#__PURE__*/React.createElement("div", {
    className: "v-download__meta",
    style: {
      flexBasis: '100%'
    }
  }, f.meta)))), note && /*#__PURE__*/React.createElement("div", {
    className: "v-download__meta"
  }, note));
}
Object.assign(__ds_scope, { DownloadCard });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/marketing/DownloadCard.jsx", error: String((e && e.message) || e) }); }

// components/marketing/FeatureRow.jsx
try { (() => {
function FeatureRow({
  title,
  kicker,
  body,
  bullets,
  media,
  flip
}) {
  return /*#__PURE__*/React.createElement("div", {
    className: 'v-feature-row' + (flip ? ' v-feature-row--flip' : '')
  }, /*#__PURE__*/React.createElement("div", {
    className: "v-feature-row__text"
  }, kicker && /*#__PURE__*/React.createElement("p", {
    className: "v-kicker"
  }, kicker), /*#__PURE__*/React.createElement("h3", null, title), body && /*#__PURE__*/React.createElement("p", null, body), bullets && /*#__PURE__*/React.createElement("ul", null, bullets.map((b, i) => /*#__PURE__*/React.createElement("li", {
    key: i
  }, /*#__PURE__*/React.createElement("span", null, b))))), /*#__PURE__*/React.createElement("div", null, media));
}
Object.assign(__ds_scope, { FeatureRow });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/marketing/FeatureRow.jsx", error: String((e && e.message) || e) }); }

// components/marketing/FeatureSection.jsx
try { (() => {
function FeatureSection({
  id,
  kicker,
  title,
  sub,
  center,
  features,
  children
}) {
  return /*#__PURE__*/React.createElement("section", {
    className: 'v-section' + (center ? ' v-section--center' : ''),
    id: id
  }, /*#__PURE__*/React.createElement("div", {
    className: "v-wrap"
  }, kicker && /*#__PURE__*/React.createElement("p", {
    className: "v-kicker"
  }, kicker), title && /*#__PURE__*/React.createElement("h2", {
    className: "v-section__title"
  }, title), sub && /*#__PURE__*/React.createElement("p", {
    className: "v-section__sub"
  }, sub), features && /*#__PURE__*/React.createElement("div", {
    className: "v-feature-grid",
    style: center ? {
      textAlign: 'left'
    } : undefined
  }, features.map(f => /*#__PURE__*/React.createElement("article", {
    className: "v-feature",
    key: f.title
  }, f.icon && /*#__PURE__*/React.createElement("div", {
    className: "v-feature__icon"
  }, /*#__PURE__*/React.createElement(__ds_scope.Icon, {
    name: f.icon,
    size: 18
  })), /*#__PURE__*/React.createElement("h3", null, f.title), /*#__PURE__*/React.createElement("p", null, f.body), f.look && /*#__PURE__*/React.createElement("p", {
    className: "v-feature__look"
  }, f.look)))), children));
}
Object.assign(__ds_scope, { FeatureSection });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/marketing/FeatureSection.jsx", error: String((e && e.message) || e) }); }

// components/marketing/Hero.jsx
try { (() => {
function Hero({
  pill,
  title,
  lede,
  actions,
  fine,
  align = 'center',
  children
}) {
  return /*#__PURE__*/React.createElement("section", {
    className: 'v-hero' + (align === 'left' ? ' v-hero--left' : '')
  }, /*#__PURE__*/React.createElement("div", {
    className: "v-wrap"
  }, pill && /*#__PURE__*/React.createElement("a", {
    className: "v-pill",
    href: pill.href || '#'
  }, /*#__PURE__*/React.createElement("b", null, pill.tag), /*#__PURE__*/React.createElement("span", {
    className: "v-pill__label"
  }, pill.label)), /*#__PURE__*/React.createElement("h1", {
    className: "v-hero__title"
  }, title), lede && /*#__PURE__*/React.createElement("p", {
    className: "v-hero__lede"
  }, lede), actions && /*#__PURE__*/React.createElement("div", {
    className: "v-hero__cta"
  }, actions), fine && /*#__PURE__*/React.createElement("p", {
    className: "v-hero__fine"
  }, fine)), children);
}
Object.assign(__ds_scope, { Hero });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/marketing/Hero.jsx", error: String((e && e.message) || e) }); }

// components/marketing/ProofPoints.jsx
try { (() => {
function ProofPoints({
  items = []
}) {
  return /*#__PURE__*/React.createElement("div", {
    className: "v-stats"
  }, items.map((s, i) => /*#__PURE__*/React.createElement("div", {
    className: "v-stat",
    key: i
  }, /*#__PURE__*/React.createElement("div", {
    className: "v-stat__value"
  }, s.value, s.unit && /*#__PURE__*/React.createElement("small", null, s.unit)), /*#__PURE__*/React.createElement("div", {
    className: "v-stat__label"
  }, s.label), s.detail && /*#__PURE__*/React.createElement("div", {
    className: "v-stat__detail"
  }, s.detail))));
}
Object.assign(__ds_scope, { ProofPoints });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/marketing/ProofPoints.jsx", error: String((e && e.message) || e) }); }

// components/navigation/Footer.jsx
try { (() => {
function Footer({
  about,
  columns = [],
  base
}) {
  return /*#__PURE__*/React.createElement("footer", {
    className: "v-footer"
  }, /*#__PURE__*/React.createElement("div", {
    className: "v-wrap"
  }, /*#__PURE__*/React.createElement("div", {
    className: "v-footer__grid"
  }, /*#__PURE__*/React.createElement("div", {
    className: "v-footer__about"
  }, about), columns.map(c => /*#__PURE__*/React.createElement("div", {
    key: c.title
  }, /*#__PURE__*/React.createElement("h4", null, c.title), /*#__PURE__*/React.createElement("ul", null, c.links.map(l => /*#__PURE__*/React.createElement("li", {
    key: l.label
  }, /*#__PURE__*/React.createElement("a", {
    href: l.href
  }, l.label))))))), base && /*#__PURE__*/React.createElement("div", {
    className: "v-footer__base"
  }, base)));
}
Object.assign(__ds_scope, { Footer });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/navigation/Footer.jsx", error: String((e && e.message) || e) }); }

// components/navigation/Tabs.jsx
try { (() => {
function Tabs({
  tabs = [],
  value,
  onChange,
  variant = 'segmented',
  center,
  label = 'Tabs',
  idPrefix = 'tab'
}) {
  const [own, setOwn] = React.useState(tabs[0] && tabs[0].id);
  const cur = value ?? own;
  const set = id => {
    onChange ? onChange(id) : setOwn(id);
  };
  const refs = React.useRef({});
  const onKey = (e, i) => {
    const d = e.key === 'ArrowRight' ? 1 : e.key === 'ArrowLeft' ? -1 : 0;
    if (!d) return;
    const n = tabs[(i + d + tabs.length) % tabs.length];
    set(n.id);
    refs.current[n.id] && refs.current[n.id].focus();
  };
  const active = tabs.find(t => t.id === cur);
  return /*#__PURE__*/React.createElement("div", {
    className: "v-tabs"
  }, /*#__PURE__*/React.createElement("div", {
    role: "tablist",
    "aria-label": label,
    className: ['v-tabs__list', variant === 'line' && 'v-tabs__list--line', center && 'v-tabs__list--center'].filter(Boolean).join(' ')
  }, tabs.map((t, i) => /*#__PURE__*/React.createElement("button", {
    key: t.id,
    ref: el => refs.current[t.id] = el,
    role: "tab",
    id: idPrefix + '-' + t.id,
    "aria-controls": idPrefix + '-panel',
    "aria-selected": t.id === cur,
    tabIndex: t.id === cur ? 0 : -1,
    className: "v-tabs__tab",
    onClick: () => set(t.id),
    onKeyDown: e => onKey(e, i)
  }, t.label))), active && active.content !== undefined && /*#__PURE__*/React.createElement("div", {
    role: "tabpanel",
    id: idPrefix + '-panel',
    "aria-labelledby": idPrefix + '-' + cur,
    className: "v-tabs__panel"
  }, active.content));
}
Object.assign(__ds_scope, { Tabs });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/navigation/Tabs.jsx", error: String((e && e.message) || e) }); }

// components/navigation/TopNav.jsx
try { (() => {
function TopNav({
  links = [],
  actions,
  logoSrc,
  brand = 'Volna',
  brandHref = '#',
  onNavigate
}) {
  const click = l => e => {
    if (onNavigate) {
      e.preventDefault();
      onNavigate(l);
    }
  };
  return /*#__PURE__*/React.createElement("header", {
    className: "v-nav"
  }, /*#__PURE__*/React.createElement("div", {
    className: "v-wrap v-nav__bar"
  }, /*#__PURE__*/React.createElement("a", {
    className: "v-brand",
    href: brandHref,
    onClick: onNavigate ? e => {
      e.preventDefault();
      onNavigate({
        href: brandHref,
        home: true
      });
    } : undefined
  }, logoSrc && /*#__PURE__*/React.createElement("img", {
    src: logoSrc,
    alt: ""
  }), brand), /*#__PURE__*/React.createElement("nav", {
    className: "v-nav__links",
    "aria-label": "Main"
  }, links.map(l => /*#__PURE__*/React.createElement("a", {
    key: l.href,
    href: l.href,
    "aria-current": l.current ? 'page' : undefined,
    onClick: click(l)
  }, l.label))), /*#__PURE__*/React.createElement("div", {
    className: "v-nav__end"
  }, actions, /*#__PURE__*/React.createElement("details", {
    className: "v-nav__menu"
  }, /*#__PURE__*/React.createElement("summary", {
    className: "v-btn v-btn--ghost v-btn--sm v-btn--icon",
    "aria-label": "Menu"
  }, /*#__PURE__*/React.createElement(__ds_scope.Icon, {
    name: "menu"
  })), /*#__PURE__*/React.createElement("nav", {
    className: "v-nav__sheet",
    "aria-label": "Main"
  }, links.map(l => /*#__PURE__*/React.createElement("a", {
    key: l.href,
    href: l.href,
    "aria-current": l.current ? 'page' : undefined,
    onClick: click(l)
  }, l.label)))))));
}
Object.assign(__ds_scope, { TopNav });
})(); } catch (e) { __ds_ns.__errors.push({ path: "components/navigation/TopNav.jsx", error: String((e && e.message) || e) }); }

__ds_ns.Button = __ds_scope.Button;

__ds_ns.TextLink = __ds_scope.TextLink;

__ds_ns.Callout = __ds_scope.Callout;

__ds_ns.Chip = __ds_scope.Chip;

__ds_ns.CodeBlock = __ds_scope.CodeBlock;

__ds_ns.Kbd = __ds_scope.Kbd;

__ds_ns.Icon = __ds_scope.Icon;

__ds_ns.ThemeSwitch = __ds_scope.ThemeSwitch;

__ds_ns.BenchTable = __ds_scope.BenchTable;

__ds_ns.ComparisonBars = __ds_scope.ComparisonBars;

__ds_ns.DocsLayout = __ds_scope.DocsLayout;

__ds_ns.DocsSidebar = __ds_scope.DocsSidebar;

__ds_ns.Toc = __ds_scope.Toc;

__ds_ns.DemoFrame = __ds_scope.DemoFrame;

__ds_ns.DownloadCard = __ds_scope.DownloadCard;

__ds_ns.FeatureRow = __ds_scope.FeatureRow;

__ds_ns.FeatureSection = __ds_scope.FeatureSection;

__ds_ns.Hero = __ds_scope.Hero;

__ds_ns.ProofPoints = __ds_scope.ProofPoints;

__ds_ns.Footer = __ds_scope.Footer;

__ds_ns.Tabs = __ds_scope.Tabs;

__ds_ns.TopNav = __ds_scope.TopNav;

})();
