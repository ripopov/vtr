/**
 * Paired bars drawn to scale: the other format at 100%, VTR at its measured share, the saving hatched.
 */
export interface BarSide { name: string; value: number; display: string; }
export interface BarRow { ratio: string; label: string; theirs: BarSide; ours: BarSide; }
export interface ComparisonBarsProps { title?: string; rows: BarRow[]; note?: React.ReactNode; }
export declare function ComparisonBars(props: ComparisonBarsProps): JSX.Element;
