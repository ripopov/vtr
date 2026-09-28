/**
 * One platform's downloads: heading, badge, files with size and checksum. current = the detected platform.
 */
export interface DownloadFile { label: string; href: string; size?: string; meta?: string; }
export interface DownloadCardProps {
  platform: string;
  icon?: string;
  badge?: string;
  description?: React.ReactNode;
  files?: DownloadFile[];
  current?: boolean;
  note?: React.ReactNode;
}
export declare function DownloadCard(props: DownloadCardProps): JSX.Element;
