import { useEffect, useState } from 'react';
import { PortfolioListResponse, fetchPortfolios } from './api';
import { PortfolioOverview } from './components/PortfolioOverview';
import { PositionDetail } from './components/PositionDetail';
import { UploadPanel } from './components/UploadPanel';

export function App() {
  const [portfolios, setPortfolios] = useState<PortfolioListResponse | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [portfolioId, setPortfolioId] = useState<string | null>(null);
  const [instrumentId, setInstrumentId] = useState<string | null>(null);
  // Incremented after each successful import; views reload when it changes.
  const [refreshKey, setRefreshKey] = useState(0);

  useEffect(() => {
    fetchPortfolios()
      .then((response) => {
        setPortfolios(response);
        setLoadError(null);
        setPortfolioId((current) => current ?? response.portfolios[0]?.portfolio_id ?? null);
      })
      .catch((e: Error) => setLoadError(e.message));
  }, [refreshKey]);

  function selectPortfolio(id: string) {
    setPortfolioId(id);
    setInstrumentId(null);
  }

  return (
    <>
      <header className="app-header">
        <div className="container">
          <h1>Portfolio Investigation Tool</h1>
          {portfolios && (
            <p>
              Valuation date <strong>{portfolios.as_of}</strong> · Dataset <code>{portfolios.dataset_id}</code>
            </p>
          )}
        </div>
      </header>

      <main className="container">
        <UploadPanel onImported={() => setRefreshKey((k) => k + 1)} />

        <section className="card">
          <h2>2. Portfolio overview</h2>
          {loadError && <div className="alert error">Could not load portfolios: {loadError}</div>}
          {!portfolios && !loadError && <p className="muted">Loading portfolios…</p>}
          {portfolios && (
            <div className="portfolio-tabs" role="tablist" aria-label="Portfolios">
              {portfolios.portfolios.map((p) => (
                <button
                  key={p.portfolio_id}
                  role="tab"
                  aria-selected={p.portfolio_id === portfolioId}
                  className={`tab ${p.portfolio_id === portfolioId ? 'active' : ''}`}
                  onClick={() => selectPortfolio(p.portfolio_id)}
                >
                  <span className="tab-name">{p.name}</span>
                  <span className="tab-meta">
                    {p.portfolio_id} · {p.trade_count} trades
                  </span>
                </button>
              ))}
            </div>
          )}
          {portfolioId && (
            <PortfolioOverview
              portfolioId={portfolioId}
              refreshKey={refreshKey}
              selectedInstrumentId={instrumentId}
              onSelectPosition={setInstrumentId}
            />
          )}
        </section>

        {portfolioId && instrumentId && (
          <PositionDetail
            portfolioId={portfolioId}
            instrumentId={instrumentId}
            refreshKey={refreshKey}
            onClose={() => setInstrumentId(null)}
          />
        )}
      </main>
    </>
  );
}
