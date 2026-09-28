Tabs for demo pickers (segmented) and install methods or docs panels (line).

```jsx
<Tabs center tabs={[{id:"waves",label:"Waves"},{id:"pipe",label:"Pipeline"}]} value={t} onChange={setT} />
<Tabs variant="line" tabs={[{id:"mac",label:"macOS",content:<CodeBlock … />}]} />
```

Arrow keys move between tabs. Omit content to use Tabs as a picker only.
