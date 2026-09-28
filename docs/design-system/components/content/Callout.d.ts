/** Tinted note box for docs: info (accent), success, warning, danger. Title is required; body optional. */
export interface CalloutProps {
  tone?: 'info' | 'success' | 'warning' | 'danger';
  title: React.ReactNode;
  icon?: string;
  children?: React.ReactNode;
}
export declare function Callout(props: CalloutProps): JSX.Element;
