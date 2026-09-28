/**
 * Button or link-button. Primary uses the accent (viewer blue); one per view.
 */
export interface ButtonProps {
  variant?: 'primary' | 'secondary' | 'ghost';
  size?: 'sm' | 'md' | 'lg';
  /** Renders an <a> when set. */
  href?: string;
  /** Lucide icon name before the label (icon-only when no children). */
  icon?: string;
  iconRight?: string;
  /** Small mono suffix, e.g. a size "12.4 MB" or a shortcut. */
  meta?: string;
  disabled?: boolean;
  children?: React.ReactNode;
  className?: string;
  onClick?: (e: React.MouseEvent) => void;
  'aria-label'?: string;
}
export declare function Button(props: ButtonProps): JSX.Element;
