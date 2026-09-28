Window for the live WASM demo (iframe) or a real product screenshot.

```jsx
<DemoFrame src="https://…/web/index.html?file=landing.vtr" path="landing.vtr" caption="…" />
<DemoFrame image="assets/screens/volna-waves.png" path="rsa256.vtr" />
```

16:10 by default (aspect prop). Shows a loading overlay until the iframe loads; state="missing" shows build instructions.
