/** Light / dark / system segmented switch. Uncontrolled: writes html[data-theme] and localStorage "volna-theme". */
export interface ThemeSwitchProps {
  value?: 'light' | 'dark' | 'system';
  onChange?: (mode: 'light' | 'dark' | 'system') => void;
  label?: string;
}
export declare function ThemeSwitch(props: ThemeSwitchProps): JSX.Element;
