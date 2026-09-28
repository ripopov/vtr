/** Site footer: an about blurb, link columns and a base line (licence, version). */
export interface FooterColumn { title: string; links: { label: string; href: string }[]; }
export interface FooterProps {
  about?: React.ReactNode;
  columns?: FooterColumn[];
  base?: React.ReactNode;
}
export declare function Footer(props: FooterProps): JSX.Element;
