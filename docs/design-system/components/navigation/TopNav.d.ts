/**
 * Sticky, translucent top bar: brand, section links, trailing actions. Links collapse into a menu under 860px.
 */
export interface NavLink { label: string; href: string; current?: boolean; }
export interface TopNavProps {
  links?: NavLink[];
  /** Right-aligned controls; wrap secondary ones in className="v-hide-sm" to drop them on phones. */
  actions?: React.ReactNode;
  /** Path to assets/logo/volna.svg relative to the page. */
  logoSrc?: string;
  brand?: string;
  brandHref?: string;
  /** Intercept clicks (single-page prototypes). */
  onNavigate?: (link: NavLink & { home?: boolean }) => void;
}
export declare function TopNav(props: TopNavProps): JSX.Element;
