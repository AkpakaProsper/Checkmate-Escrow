import { useState, useEffect } from 'react'
import { useWallet } from './hooks/useWallet'
import { ThemeToggle } from './components/ThemeToggle'
import { WalletConnector } from './components/wallet/WalletConnector'
import { AdminPanel } from './pages/AdminPanel'
import { MatchDetailPage } from './pages/MatchDetailPage'
import { MatchList } from './components/MatchList'
import { CreateMatchForm } from './components/match/CreateMatchForm'
import { ThemeToggle } from './components/ThemeToggle'
import './App.css'

/** Matches deep-links of the form /match/1234 */
const MATCH_ROUTE = /^\/match\/(\d+)$/

interface Match {
  matchId: number
  player1: string
  player2: string
  stakeAmount: string
  token: string
  status: 'pending' | 'active' | 'completed' | 'cancelled'
  platform: 'lichess' | 'chessdotcom'
}

function App() {
  const wallet = useWallet()
  const [matches, setMatches] = useState<Match[]>([])
  const [loading, setLoading] = useState(false)
  const isAdmin = new URLSearchParams(window.location.search).get('admin') === '1'
    || window.location.pathname === '/admin'
  const pathname = window.location.pathname

  const matchRouteMatch = pathname.match(MATCH_ROUTE)

  useEffect(() => {
    if (pathname === '/matches' && wallet.connected) {
      setLoading(true)
      // TODO: Fetch matches from API
      setLoading(false)
    }
  }, [pathname, wallet.connected])

  if (isAdmin) {
    return <AdminPanel wallet={wallet} />
  }

  if (matchRouteMatch) {
    return <MatchDetailPage matchId={Number(matchRouteMatch[1])} />
  }

  if (pathname === '/matches') {
    return (
      <main id="matches">
        <ThemeToggle />
        <h1>Matches</h1>
        {!wallet.connected ? (
          <div>
            <p>Connect your wallet to view matches.</p>
            <WalletConnector wallet={wallet} />
          </div>
        ) : (
          <MatchList matches={matches} loading={loading} />
        )}
      </main>
    )
  }

  if (pathname === '/create') {
    return (
      <main id="create">
        <ThemeToggle />
        <h1>Create Match</h1>
        {!wallet.connected ? (
          <div>
            <p>Connect your wallet to create a match.</p>
            <WalletConnector wallet={wallet} />
          </div>
        ) : (
          <CreateMatchForm key={wallet.publicKey} onSubmit={() => {
            // TODO: Handle form submission
          }} />
        )}
      </main>
    )
  }

  return (
    <main id="landing">
      <ThemeToggle />
      <h1>Checkmate-Escrow</h1>
      <p className="tagline">Trustless chess wagering on Stellar — stake, play, get paid instantly.</p>
      <WalletConnector wallet={wallet} />
      {wallet.connected && (
        <nav>
          <a href="/matches">Browse Matches</a>
          <a href="/create">Create Match</a>
        </nav>
      )}
    </main>
  )
}

export default App
