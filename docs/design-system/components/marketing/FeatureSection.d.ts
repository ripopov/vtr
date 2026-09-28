/**
 * Section with kicker, title, sub and either a grid of feature cards or FeatureRow children (text beside a screenshot).
 */
export interface Feature { icon?: string; title: string; body: React.ReactNode; look?: string; }
export interface FeatureSectionProps {
  id?: string;
  kicker?: string;
  title?: React.ReactNode;
  sub?: React.ReactNode;
  center?: boolean;
  features?: Feature[];
  children?: React.ReactNode;
}
export declare function FeatureSection(props: FeatureSectionProps): JSX.Element;
