/** Grouped docs navigation for DocsLayout's sidebar slot. */
export interface DocsNavItem { label: string; href: string; current?: boolean; }
export interface DocsSidebarProps {
  groups: { title: string; items: DocsNavItem[] }[];
  onNavigate?: (item: DocsNavItem) => void;
  label?: string;
}
export declare function DocsSidebar(props: DocsSidebarProps): JSX.Element;
