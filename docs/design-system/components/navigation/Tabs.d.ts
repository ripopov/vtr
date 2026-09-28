/** Accessible tabs (arrow keys move). Segmented pill style for demo pickers and install methods; line style inside docs. */
export interface TabItem { id: string; label: React.ReactNode; content?: React.ReactNode; }
export interface TabsProps {
  tabs: TabItem[];
  value?: string;
  onChange?: (id: string) => void;
  variant?: 'segmented' | 'line';
  center?: boolean;
  label?: string;
  idPrefix?: string;
}
export declare function Tabs(props: TabsProps): JSX.Element;
