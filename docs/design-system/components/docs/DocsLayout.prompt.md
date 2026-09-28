Three-column docs page shell: sidebar, prose, on-page TOC.

```jsx
<DocsLayout sidebar={<DocsSidebar groups={…} />} toc={<Toc items={…} />} crumbs={[{label:"Docs",href:"/docs"}]}>
  <h1>…</h1>
</DocsLayout>
```

Children render inside .v-prose (68ch). TOC hides <1180px; sidebar becomes a disclosure <820px.
