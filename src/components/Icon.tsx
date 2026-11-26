import type { ReactNode, SVGProps } from 'react'

export type IconName =
  | 'dashboard'
  | 'nodes'
  | 'rulesets'
  | 'activity'
  | 'logs'
  | 'search'
  | 'refresh'
  | 'chevron-down'
  | 'chevron-right'
  | 'chevron-up'
  | 'plus'
  | 'trash'
  | 'pause'
  | 'play'
  | 'edit'
  | 'close'
  | 'minimize'
  | 'maximize'
  | 'check'
  | 'download'
  | 'upload'
  | 'alert'
  | 'arrow-up'
  | 'arrow-down'
  | 'dots'
  | 'grip'

const paths: Record<IconName, ReactNode> = {
  dashboard: (
    <>
      <path d="m3 10 9-7 9 7" />
      <path d="M5 9v10a1 1 0 0 0 1 1h4v-6h4v6h4a1 1 0 0 0 1-1V9" />
    </>
  ),
  nodes: (
    <>
      <rect x="3" y="4" width="18" height="6" rx="1.5" />
      <rect x="3" y="14" width="18" height="6" rx="1.5" />
      <path d="M7 7h.01M7 17h.01" />
    </>
  ),
  rulesets: (
    <>
      <path d="m12 3 9 5-9 5-9-5 9-5Z" />
      <path d="m3 12 9 5 9-5" />
      <path d="m3 16 9 5 9-5" />
    </>
  ),
  activity: (
    <>
      <circle cx="5.5" cy="6" r="2" />
      <circle cx="18.5" cy="8" r="2" />
      <circle cx="8" cy="18" r="2" />
      <circle cx="18" cy="18" r="2" />
      <path d="m7.3 7.1 9.2.6M6.4 7.9l1 8.1m2.5 1.2 6.2-1.1M18.2 9.8l-.1 6.2" />
    </>
  ),
  logs: (
    <>
      <rect x="4" y="3" width="16" height="18" rx="2" />
      <path d="M8 8h8M8 12h8M8 16h5" />
    </>
  ),
  search: (
    <>
      <circle cx="10.8" cy="10.8" r="6.8" />
      <path d="m16 16 4.5 4.5" />
    </>
  ),
  refresh: (
    <>
      <path d="M20 7v5h-5" />
      <path d="M18.5 16a8 8 0 1 1 .8-8.4L20 12" />
    </>
  ),
  'chevron-down': <path d="m6 9 6 6 6-6" />,
  'chevron-right': <path d="m9 6 6 6-6 6" />,
  'chevron-up': <path d="m6 15 6-6 6 6" />,
  plus: <path d="M12 5v14M5 12h14" />,
  trash: (
    <>
      <path d="M4 7h16M9 3h6l1 4H8l1-4ZM7 7l1 14h8l1-14" />
      <path d="M10 11v6M14 11v6" />
    </>
  ),
  pause: (
    <>
      <path d="M9 5v14M15 5v14" />
    </>
  ),
  play: <path d="m8 5 11 7-11 7V5Z" />,
  edit: (
    <>
      <path d="m4 20 4.2-1 11-11a2.1 2.1 0 0 0-3-3l-11 11L4 20Z" />
      <path d="m14.8 6.2 3 3" />
    </>
  ),
  close: <path d="m6 6 12 12M18 6 6 18" />,
  minimize: <path d="M5 12h14" />,
  maximize: <rect x="6" y="6" width="12" height="12" rx="1" />,
  check: <path d="m5 12 4.5 4.5L19 7" />,
  download: (
    <>
      <path d="M12 3v12" />
      <path d="m7 10 5 5 5-5M5 21h14" />
    </>
  ),
  upload: (
    <>
      <path d="M12 17V5" />
      <path d="m7 10 5-5 5 5M5 21h14" />
    </>
  ),
  alert: (
    <>
      <path d="M12 3 2.5 20h19L12 3Z" />
      <path d="M12 9v5M12 17h.01" />
    </>
  ),
  'arrow-up': <path d="M12 19V5m-6 6 6-6 6 6" />,
  'arrow-down': <path d="M12 5v14m-6-6 6 6 6-6" />,
  dots: (
    <>
      <circle cx="5" cy="12" r="1" />
      <circle cx="12" cy="12" r="1" />
      <circle cx="19" cy="12" r="1" />
    </>
  ),
  grip: (
    <>
      <circle cx="9" cy="6" r="1.2" fill="currentColor" stroke="none" />
      <circle cx="15" cy="6" r="1.2" fill="currentColor" stroke="none" />
      <circle cx="9" cy="12" r="1.2" fill="currentColor" stroke="none" />
      <circle cx="15" cy="12" r="1.2" fill="currentColor" stroke="none" />
      <circle cx="9" cy="18" r="1.2" fill="currentColor" stroke="none" />
      <circle cx="15" cy="18" r="1.2" fill="currentColor" stroke="none" />
    </>
  ),
}

export function Icon({
  name,
  size = 20,
  ...props
}: { name: IconName; size?: number } & Omit<SVGProps<SVGSVGElement>, 'name'>) {
  return (
    <svg
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
      {...props}
    >
      {paths[name]}
    </svg>
  )
}

export function Logo({ size = 34 }: { size?: number }) {
  return (
    <svg viewBox="0 0 1024 1024" width={size} height={size} aria-hidden="true">
      <path d="M512 108 800 218v258c0 196-108 336-288 434-180-98-288-238-288-434V218l288-110Z" fill="#78a9ff" />
      <path d="M512 176 736 262v214c0 155-82 270-224 356-142-86-224-201-224-356V262l224-86Z" fill="#101318" />
      <circle cx="512" cy="422" r="68" fill="#78a9ff" />
      <circle cx="424" cy="566" r="50" fill="#78a9ff" />
      <circle cx="600" cy="566" r="50" fill="#78a9ff" />
      <circle cx="512" cy="660" r="42" fill="#b99bff" />
      <path d="M512 422 424 566M512 422l88 144M424 566l88 94M600 566l-88 94" stroke="#78a9ff" strokeWidth="26" strokeLinecap="round" />
    </svg>
  )
}
