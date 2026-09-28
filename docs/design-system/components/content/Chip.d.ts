/** Small mono label: file sections, signal names, formats, platform badges. */
export interface ChipProps {
  tone?: 'default' | 'accent' | 'warning' | 'danger';
  /** CSS colour for a leading square, e.g. "var(--wave-signal)". */
  swatch?: string;
  children: React.ReactNode;
}
export declare function Chip(props: ChipProps): JSX.Element;
