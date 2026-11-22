import type { RuntimeView } from '../types'
import { Icon, Logo, type IconName } from './Icon'

export type Page = 'dashboard' | 'nodes' | 'rulesets' | 'connections' | 'logs'

const navigation: Array<{ page: Page; label: string; icon: IconName }> = [
  { page: 'dashboard', label: 'Dashboard', icon: 'dashboard' },
  { page: 'nodes', label: 'Nodes', icon: 'nodes' },
  { page: 'rulesets', label: 'Rulesets', icon: 'rulesets' },
  { page: 'connections', label: 'Live Connections', icon: 'activity' },
  { page: 'logs', label: 'Logs', icon: 'logs' },
]

export function Sidebar({
  page,
  onNavigate,
  runtime,
}: {
  page: Page
  onNavigate: (page: Page) => void
  runtime: RuntimeView
}) {
  return (
    <aside className="sidebar">
      <div className="brand">
        <Logo size={36} />
        <span>Clashfork</span>
      </div>
      <nav className="nav" aria-label="Main navigation">
        {navigation.map((item) => (
          <button
            key={item.page}
            type="button"
            className={`nav-item ${page === item.page ? 'is-active' : ''}`}
            aria-current={page === item.page ? 'page' : undefined}
            aria-label={item.label}
            onClick={() => onNavigate(item.page)}
            title={item.label}
          >
            <Icon name={item.icon} size={22} />
            <span className="nav-label">{item.label}</span>
          </button>
        ))}
      </nav>
      <div className="sidebar-foot">
        <span
          className={`service-dot ${runtime.serviceAvailable ? 'is-ready' : ''}`}
          aria-hidden="true"
        />
        <span className="sidebar-foot-text">
          {runtime.serviceAvailable ? 'TUN service ready' : runtime.serviceMessage ? 'TUN service unavailable' : 'TUN service check'}
        </span>
        <span className="version">1.0.4</span>
      </div>
    </aside>
  )
}
