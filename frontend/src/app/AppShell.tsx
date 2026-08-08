import type { ReactNode } from 'react'

export type Destination = 'home' | 'events' | 'more'

export type AppView =
  | { kind: 'destination'; destination: Destination }
  | { kind: 'camera'; returnTo: Destination; archiveTarget?: { cameraId: string; archiveAt: string } }

interface AppShellProps {
  children: ReactNode
  destination: Destination
  header: ReactNode
  onNavigate(destination: Destination): void
  status?: ReactNode
}

const destinations = [
  { id: 'home', label: 'Главная' },
  { id: 'events', label: 'События' },
  { id: 'more', label: 'Ещё' },
] as const

function DestinationIcon({ destination }: { destination: Destination }) {
  if (destination === 'home') return <svg aria-hidden="true" fill="none" height="24" stroke="currentColor" strokeWidth="1.8" viewBox="0 0 24 24" width="24"><path d="M4 10.5 12 4l8 6.5V20h-5v-6H9v6H4Z" /></svg>
  if (destination === 'events') return <svg aria-hidden="true" fill="none" height="24" stroke="currentColor" strokeWidth="1.8" viewBox="0 0 24 24" width="24"><path d="M6 3h12v18H6zM9 8h6M9 12h6M9 16h4" /></svg>
  return <svg aria-hidden="true" fill="currentColor" height="24" viewBox="0 0 24 24" width="24"><circle cx="5" cy="12" r="2" /><circle cx="12" cy="12" r="2" /><circle cx="19" cy="12" r="2" /></svg>
}

export function AppShell({ children, destination, header, onNavigate, status }: AppShellProps) {
  return <div className="app-shell">
    <a className="skip-link" href="#main-content">К основному содержимому</a>
    <header className="shell-header">{header}</header>
    {status && <div className="shell-status">{status}</div>}
    <main className="app-content shell-content" id="main-content">{children}</main>
    <nav aria-label="Основная навигация" className="bottom-nav shell-navigation">
      {destinations.map((item) => <button
        aria-current={destination === item.id ? 'page' : undefined}
        className={destination === item.id ? 'active' : ''}
        key={item.id}
        onClick={() => onNavigate(item.id)}
        type="button"
      >
        <DestinationIcon destination={item.id} />
        <span>{item.label}</span>
      </button>)}
    </nav>
  </div>
}
