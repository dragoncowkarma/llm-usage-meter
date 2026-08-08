/** Inline 16px stroke icons — no icon package, no network fetch under the CSP. */

interface IconProps {
  className?: string;
}

function Svg({ children, className }: IconProps & { children: React.ReactNode }) {
  return (
    <svg
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className={className ?? "h-4 w-4"}
    >
      {children}
    </svg>
  );
}

export const RefreshIcon = ({ className }: IconProps) => (
  <Svg className={className}>
    <path d="M13.5 8a5.5 5.5 0 1 1-1.6-3.9" />
    <path d="M13.5 2.5v3h-3" />
  </Svg>
);

export const GearIcon = ({ className }: IconProps) => (
  <Svg className={className}>
    <circle cx="8" cy="8" r="2.1" />
    <path d="M8 1.5v1.6M8 12.9v1.6M14.5 8h-1.6M3.1 8H1.5M12.6 3.4l-1.1 1.1M4.5 11.5l-1.1 1.1M12.6 12.6l-1.1-1.1M4.5 4.5 3.4 3.4" />
  </Svg>
);

export const PowerIcon = ({ className }: IconProps) => (
  <Svg className={className}>
    <path d="M8 2v6" />
    <path d="M4.6 4.2a5 5 0 1 0 6.8 0" />
  </Svg>
);

export const ChevronIcon = ({ className }: IconProps) => (
  <Svg className={className}>
    <path d="m5 6.5 3 3 3-3" />
  </Svg>
);

export const AlertIcon = ({ className }: IconProps) => (
  <Svg className={className}>
    <path d="M8 2.5 1.8 13h12.4L8 2.5Z" />
    <path d="M8 6.6v3.1M8 11.6v.01" />
  </Svg>
);

export const FolderIcon = ({ className }: IconProps) => (
  <Svg className={className}>
    <path d="M1.8 4.2h4l1.2 1.5h7.2v6.6H1.8V4.2Z" />
  </Svg>
);
