Light / dark / system switch for the site nav.

```jsx
<ThemeSwitch />  // uncontrolled: writes html[data-theme]
<ThemeSwitch value={mode} onChange={setMode} />
```

Pair with the inline head script that sets data-theme before paint.
