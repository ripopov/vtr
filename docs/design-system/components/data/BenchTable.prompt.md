Benchmark / reference table with mono tabular numbers.

```jsx
<BenchTable columns={[{key:"w",label:"workload"},{key:"v",label:"VTR",ours:true},{key:"r",label:"vs FST",ratio:true}]} rows={[{w:"c910_coremark",v:"243.13 MiB",r:"63%"}]} caption="Machine, date" />
```

First column is the row header. ours tints the VTR column; ratio colours the comparison. Scrolls inside its frame.
