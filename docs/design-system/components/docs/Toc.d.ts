/** On-page table of contents for DocsLayout's toc slot. level 3 entries indent. */
export interface TocItem { id: string; label: string; level?: 2 | 3; }
export interface TocProps { items: TocItem[]; active?: string; title?: string; }
export declare function Toc(props: TocProps): JSX.Element;
