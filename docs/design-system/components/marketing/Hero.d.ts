/**
 * Page hero: release pill, display title, lede, CTAs and a fine-print line. Put the DemoFrame in children.
 */
export interface HeroProps {
  /** Release pill: { tag: "VTR 1.1", label: "…", href }. */
  pill?: { tag: string; label: string; href?: string };
  /** Wrap one phrase in <em> to colour it with the accent. */
  title: React.ReactNode;
  lede?: React.ReactNode;
  actions?: React.ReactNode;
  fine?: React.ReactNode;
  align?: 'center' | 'left';
  children?: React.ReactNode;
}
export declare function Hero(props: HeroProps): JSX.Element;
