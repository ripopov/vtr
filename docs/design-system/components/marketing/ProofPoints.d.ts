/** Row of measured numbers with a one-line label and a mono source detail. Numbers only from the benchmark report. */
export interface ProofPoint { value: string; unit?: string; label: string; detail?: string; }
export interface ProofPointsProps { items: ProofPoint[]; }
export declare function ProofPoints(props: ProofPointsProps): JSX.Element;
