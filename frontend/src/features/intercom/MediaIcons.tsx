const strokeProps = {
  'aria-hidden': true,
  fill: 'none',
  height: 22,
  stroke: 'currentColor',
  strokeLinecap: 'round',
  strokeLinejoin: 'round',
  strokeWidth: 1.9,
  viewBox: '0 0 24 24',
  width: 22,
} as const

export function SoundIcon({ muted }: { muted: boolean }) {
  return <svg {...strokeProps}>
    <path d="M4 9.5v5h3.6l4.9 4v-13l-4.9 4Z" />
    {muted ? <path d="m16 9.5 5 5m0-5-5 5" /> : <path d="M15.8 9.2a4 4 0 0 1 0 5.6m2.6-8.3a7.6 7.6 0 0 1 0 11" />}
  </svg>
}

export function FullscreenIcon({ active }: { active: boolean }) {
  return <svg {...strokeProps}>
    <path d={active ? 'M9 4v5H4m16 0h-5V4m0 16v-5h5M4 15h5v5' : 'M4 9V4h5m6 0h5v5m0 6v5h-5m-6 0H4v-5'} />
  </svg>
}

export function ExpandIcon() {
  return <svg {...strokeProps}>
    <path d="M14 4h6v6m0-6-7 7M10 20H4v-6m0 6 7-7" />
  </svg>
}

export function RefreshIcon() {
  return <svg {...strokeProps}>
    <path d="M19.5 12a7.5 7.5 0 1 1-2.2-5.3M19.5 4.5v4.2h-4.2" />
  </svg>
}

export function PlayIcon() {
  return <svg aria-hidden="true" fill="currentColor" height="26" viewBox="0 0 24 24" width="26">
    <path d="M8 5.6v12.8a.8.8 0 0 0 1.2.7l10-6.4a.8.8 0 0 0 0-1.4l-10-6.4a.8.8 0 0 0-1.2.7Z" />
  </svg>
}
