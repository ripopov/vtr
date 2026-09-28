/** Inline text link. */
export interface TextLinkProps {
  href: string;
  /** default = accent text, underlined; quiet = secondary text, no underline. */
  variant?: 'default' | 'quiet';
  /** Appends a mono "→". */
  arrow?: boolean;
  /** Appends "↗" and opens in a new tab. */
  external?: boolean;
  children: React.ReactNode;
}
export declare function TextLink(props: TextLinkProps): JSX.Element;
