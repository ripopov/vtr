/** Two-column feature: text + bullets beside media (usually a DemoFrame with a real screenshot). */
export interface FeatureRowProps {
  title: React.ReactNode;
  kicker?: string;
  body?: React.ReactNode;
  bullets?: React.ReactNode[];
  media?: React.ReactNode;
  /** Media on the left. */
  flip?: boolean;
}
export declare function FeatureRow(props: FeatureRowProps): JSX.Element;
