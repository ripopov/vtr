/** Lucide icon (the viewer's icon set) rendered as a CSS mask so it takes currentColor. */
export interface IconProps {
  /** Lucide icon name, e.g. "copy", "audio-waveform", "cpu", "github". */
  name: string;
  /** Pixel size. Default 16 (the viewer's icon size). */
  size?: number;
  /** Accessible label; omit for decorative icons. */
  label?: string;
  className?: string;
  style?: React.CSSProperties;
}
export declare function Icon(props: IconProps): JSX.Element;
