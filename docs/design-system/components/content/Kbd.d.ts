/** Keyboard key chip. Pass children for one key or keys=[…] for a chord. Spell platform keys: "Ctrl", "⌘", "⏎". */
export interface KbdProps { keys?: string[]; children?: React.ReactNode; }
export declare function Kbd(props: KbdProps): JSX.Element;
