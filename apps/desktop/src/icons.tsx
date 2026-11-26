// The few icons the surfaces use, drawn here: 16 px boxes, 1.5 px strokes, the current colour.

import type { ReactNode } from "react";

function Icon({ children, size = 16 }: { children: ReactNode; size?: number }) {
  return (
    <svg
      className="icon"
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.5"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {children}
    </svg>
  );
}

/** The app's mark: two chevrons, the first one fainter. */
export function Mark({ className }: { className?: string }) {
  return (
    <svg
      className={className}
      viewBox="0 0 20 20"
      fill="none"
      stroke="currentColor"
      strokeWidth="2.6"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <polyline points="4,5 9,10 4,15" opacity="0.45" />
      <polyline points="10.5,5 15.5,10 10.5,15" />
    </svg>
  );
}

export function MicIcon() {
  return (
    <Icon>
      <rect x="5.75" y="1.75" width="4.5" height="8" rx="2.25" />
      <path d="M3.25 7.5a4.75 4.75 0 0 0 9.5 0" />
      <path d="M8 12.25v2" />
    </Icon>
  );
}

export function MicOffIcon() {
  return (
    <Icon>
      <path d="M10.25 6.5V4a2.25 2.25 0 0 0-4.4-.66" />
      <path d="M5.75 6.25v1.25a2.25 2.25 0 0 0 3.6 1.8" />
      <path d="M12.75 7.5a4.7 4.7 0 0 1-.5 2.1M3.25 7.5a4.75 4.75 0 0 0 7.6 3.8" />
      <path d="M8 12.25v2" />
      <path d="M2.25 2.25l11.5 11.5" />
    </Icon>
  );
}

export function MoreIcon() {
  return (
    <Icon>
      <circle cx="3.5" cy="8" r="0.9" fill="currentColor" stroke="none" />
      <circle cx="8" cy="8" r="0.9" fill="currentColor" stroke="none" />
      <circle cx="12.5" cy="8" r="0.9" fill="currentColor" stroke="none" />
    </Icon>
  );
}

export function BackIcon() {
  return (
    <Icon>
      <polyline points="10,3.5 5.5,8 10,12.5" />
    </Icon>
  );
}

export function CloseIcon() {
  return (
    <Icon size={12}>
      <path d="M4 4l8 8M12 4l-8 8" />
    </Icon>
  );
}
