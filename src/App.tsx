import { useCallback, useEffect, useState } from 'react'
import { api, emptySnapshot, subscribeErrors, subscribeSnapshots } from './api'
import { Sidebar, type Page } from './components/Sidebar'
import { Titlebar } from './components/Titlebar'
import { Banner } from './components/ui'
import { useRateRecorder } from './lib/rateRecorder'
import { ConnectionsPage } from './pages/ConnectionsPage'
import { DashboardPage } from './pages/DashboardPage'
import { LogsPage } from './pages/LogsPage'
import { NodesPage } from './pages/NodesPage'
import { RulesetsPage } from './pages/RulesetsPage'
import type { Snapshot } from './types'

const pageTitles: Record<Page, string> = {
  dashboard: 'Dashboard',
  nodes: 'Nodes',
  rulesets: 'Rulesets',
  connections: 'Live Connections',
  logs: 'Logs',
}

function App() {
  const [page, setPage] = useState<Page>('dashboard')
  const [snapshot, setSnapshot] = useState<Snapshot>(() => emptySnapshot())
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  useRateRecorder(snapshot.runtime.vpnActive)

  const reload = useCallback(async () => {
    try {
      const next = await api.getSnapshot()
      setSnapshot(next)
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : 'The Clashfork backend is unavailable.')
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void reload()
    let unlistenSnapshot: (() => void) | null = null
    let unlistenError: (() => void) | null = null
    void subscribeSnapshots((next) => {
      setSnapshot(next)
      setLoading(false)
    }).then((unlisten) => {
      unlistenSnapshot = unlisten
    })
    void subscribeErrors((message) => {
      setError(message)
      void reload()
    }).then((unlisten) => {
      unlistenError = unlisten
    })
    return () => {
      unlistenSnapshot?.()
      unlistenError?.()
    }
  }, [reload])

  const reportError = useCallback((caught: unknown) => {
    setError(caught instanceof Error ? caught.message : String(caught))
  }, [])

  const updateSnapshot = useCallback((next: Snapshot) => {
    setSnapshot(next)
    setError(null)
  }, [])

  const activePage = {
    dashboard: (
      <DashboardPage
        snapshot={snapshot}
        onSnapshot={updateSnapshot}
        onError={reportError}
      />
    ),
    nodes: (
      <NodesPage snapshot={snapshot} onSnapshot={updateSnapshot} onError={reportError} />
    ),
    rulesets: (
      <RulesetsPage snapshot={snapshot} onSnapshot={updateSnapshot} onError={reportError} />
    ),
    connections: <ConnectionsPage snapshot={snapshot} />,
    logs: (
      <LogsPage snapshot={snapshot} onSnapshot={updateSnapshot} onError={reportError} />
    ),
  }[page]

  return (
    <div className="app-shell">
      <Titlebar />
      <Sidebar page={page} onNavigate={setPage} runtime={snapshot.runtime} />
      <main className="main" aria-label={pageTitles[page]}>
        {error ? (
          <Banner onDismiss={() => setError(null)}>
            <span>{error}</span>
          </Banner>
        ) : null}
        {loading ? <div className="loading-shell" aria-label="Loading Clashfork" /> : null}
        <div className={loading ? 'page is-loading' : 'page'}>{activePage}</div>
      </main>
    </div>
  )
}

export default App
