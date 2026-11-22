import { useMemo, useState } from 'react'
import { useLive } from '../hooks'
import { formatRate } from '../lib/format'
import type { Snapshot } from '../types'
import { Icon } from '../components/Icon'
import { EmptyState, PageHeader, Panel, Toolbar } from '../components/ui'

export function ConnectionsPage({ snapshot }: { snapshot: Snapshot }) {
  const live = useLive(true)
  const [search, setSearch] = useState('')
  const [routeFilter, setRouteFilter] = useState('all')

  const routes = useMemo(
    () => [...new Set(live.connections.map((connection) => connection.route))].sort(),
    [live.connections],
  )

  const filtered = live.connections.filter((connection) => {
    const query = search.trim().toLowerCase()
    if (routeFilter !== 'all' && connection.route !== routeFilter) return false
    if (!query) return true
    return [connection.app, connection.destination, connection.route, connection.matchedGroup]
      .filter(Boolean)
      .some((value) => value!.toLowerCase().includes(query))
  })

  return (
    <div>
      <PageHeader
        title="Live Connections"
        actions={
          <span className={`connection-pill ${live.active ? 'is-connected' : ''}`}>
            <span className="status-dot" aria-hidden="true" />
            {live.active ? `${live.connections.length} active` : 'Disconnected'}
          </span>
        }
      />
      <Toolbar className="filters-row">
        <label className="search-field">
          <Icon name="search" size={18} />
          <span className="sr-only">Search app or destination</span>
          <input value={search} placeholder="Search app or destination" onChange={(event) => setSearch(event.target.value)} />
        </label>
        <label className="select-filter">
          <span className="sr-only">Route filter</span>
          <select value={routeFilter} onChange={(event) => setRouteFilter(event.target.value)}>
            <option value="all">All routes</option>
            {routes.map((route) => <option value={route} key={route}>{route}</option>)}
          </select>
        </label>
        <span className="toolbar-meta">{filtered.length} shown</span>
      </Toolbar>

      <Panel className="connections-panel">
        {!snapshot.runtime.vpnActive || live.connections.length === 0 ? (
          <EmptyState
            title={snapshot.runtime.vpnActive ? 'No active flows' : 'VPN is off'}
            description={
              snapshot.runtime.vpnActive
                ? 'Connections appear here as apps send traffic through the TUN adapter.'
                : 'Turn the VPN on from the Dashboard to see live connections and routes.'
            }
          />
        ) : (
          <table className="connections-table">
            <thead>
              <tr>
                <th scope="col">App</th>
                <th scope="col">Destination</th>
                <th scope="col">Route</th>
                <th scope="col" className="numeric">Download</th>
                <th scope="col" className="numeric">Upload</th>
              </tr>
            </thead>
            <tbody>
              {filtered.map((connection) => (
                <tr key={connection.appKey}>
                  <td><strong>{connection.app}</strong></td>
                  <td className="destination-cell">{connection.destination}<span>{connection.network.toUpperCase()}</span></td>
                  <td>
                    <span className="route-value">{connection.route}</span>
                    {connection.matchedGroup ? <span className="route-group">{connection.matchedGroup}</span> : null}
                  </td>
                  <td className="numeric rate-cell">{formatRate(connection.downloadRate)}</td>
                  <td className="numeric rate-cell">{formatRate(connection.uploadRate)}</td>
                </tr>
              ))}
              {filtered.length === 0 ? (
                <tr><td colSpan={5} className="table-empty">No connections match this filter.</td></tr>
              ) : null}
            </tbody>
          </table>
        )}
      </Panel>
    </div>
  )
}
