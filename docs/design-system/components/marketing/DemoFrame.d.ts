/**
 * Window for the live WASM demo (iframe, 16:10) or a real product screenshot. Title bar shows the trace path and links.
 */
export interface DemoFrameProps {
  /** iframe URL of the WASM viewer. */
  src?: string;
  /** Screenshot path, used when there is no src. */
  image?: string;
  alt?: string;
  title?: string;
  /** Mono path in the title bar, e.g. "landing.vtr". */
  path?: string;
  actions?: { label: string; href: string }[];
  caption?: React.ReactNode;
  state?: 'loading' | 'ready' | 'missing';
  aspect?: string;
  missing?: React.ReactNode;
}
export declare function DemoFrame(props: DemoFrameProps): JSX.Element;
