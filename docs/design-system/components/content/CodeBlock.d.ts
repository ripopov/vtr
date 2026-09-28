/**
 * Code block with filename/lang header and a copy button. lang="sh": "$ " lines are commands (copy takes only those), "#" lines are comments.
 */
export interface CodeBlockProps {
  code?: string;
  lang?: 'sh' | 'rust' | 'c' | 'sv' | 'json' | string;
  filename?: string;
  copy?: boolean;
  /** Hide the header. */
  bare?: boolean;
  /** Pre-tokenised content: spans with className tok-c / tok-k / tok-s / tok-n / tok-f / tok-p. */
  children?: React.ReactNode;
}
export declare function CodeBlock(props: CodeBlockProps): JSX.Element;
