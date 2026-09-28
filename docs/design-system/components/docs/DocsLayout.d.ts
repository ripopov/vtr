/**
 * Three-column docs page: sidebar | prose | on-page TOC. TOC drops under 1180px; sidebar folds into a disclosure under 820px.
 */
export interface DocsLayoutProps {
  sidebar?: React.ReactNode;
  toc?: React.ReactNode;
  crumbs?: { label: string; href?: string }[];
  pager?: { prev?: { label: string; href: string }; next?: { label: string; href: string } };
  children: React.ReactNode;
}
export declare function DocsLayout(props: DocsLayoutProps): JSX.Element;
