/**
 * Benchmark table: mono tabular numbers right-aligned, VTR column tinted, ratio column in accent. Scrolls inside its frame on narrow screens.
 */
export interface BenchColumn { key: string; label: string; ours?: boolean; ratio?: boolean; }
export interface BenchTableProps {
  columns: BenchColumn[];
  rows: Record<string, React.ReactNode>[];
  /** Methodology / machine note under the table. */
  caption?: React.ReactNode;
  label?: string;
}
export declare function BenchTable(props: BenchTableProps): JSX.Element;
