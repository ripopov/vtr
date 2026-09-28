Sticky translucent site header with brand, section links and actions.

```jsx
<TopNav logoSrc="volna/volna/assets/app-icon/volna.svg" links={[{label:"Docs",href:"/docs",current:true}]} actions={<Button size="sm" variant="primary">Download</Button>} />
```

Links collapse into a <details> menu under 860px; wrap non-essential actions in className="v-hide-sm".
