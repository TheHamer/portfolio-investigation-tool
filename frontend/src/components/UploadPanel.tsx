import { useState } from 'react';
import { ApiError, ImportResult, uploadTrades } from '../api';

interface Props {
  /** Called after a successful import so portfolio figures can refresh. */
  onImported: () => void;
}

interface UploadError {
  message: string;
  /** False only when the server answered with a definite failure. */
  outcomeUnknown: boolean;
}

export function UploadPanel({ onImported }: Props) {
  const [file, setFile] = useState<File | null>(null);
  const [uploading, setUploading] = useState(false);
  const [result, setResult] = useState<ImportResult | null>(null);
  const [error, setError] = useState<UploadError | null>(null);

  async function handleUpload() {
    if (!file) return;

    setUploading(true);
    setResult(null);
    setError(null);

    try {
      const outcome = await uploadTrades(file);
      setResult(outcome);
      if (outcome.outcome === 'success') onImported();
    } catch (e) {
      setError({
        message: e instanceof Error ? e.message : 'Upload failed.',
        outcomeUnknown: !(e instanceof ApiError) || e.outcomeUnknown,
      });
    } finally {
      setUploading(false);
    }
  }

  return (
    <section className="card">
      <h2>1. Upload trades</h2>
      <p className="muted">
        Upload a trade CSV with the header{' '}
        <code>trade_id,portfolio_id,instrument_id,timestamp,side,quantity,price</code>. A file is imported only if every
        row is valid; rows that duplicate a stored trade or an earlier row are skipped.
      </p>

      <div className="upload-row">
        <label className="file-picker">
          <input
            type="file"
            accept=".csv,text/csv"
            onChange={(e) => {
              setFile(e.target.files?.[0] ?? null);
              setResult(null);
              setError(null);
            }}
          />
          <span className="button secondary">Choose CSV file</span>
          <span className="file-name">{file ? file.name : 'No file selected'}</span>
        </label>
        <button className="button" onClick={handleUpload} disabled={!file || uploading}>
          {uploading ? 'Uploading…' : 'Upload'}
        </button>
      </div>

      <div aria-live="polite">
        {error &&
          (error.outcomeUnknown ? (
            <div className="alert error">
              <strong>Upload could not be confirmed.</strong> {error.message} Check the portfolio or try uploading the
              file again. Previously imported trades will be skipped.
            </div>
          ) : (
            <div className="alert error">
              <strong>Upload failed.</strong> {error.message} Nothing was imported.
            </div>
          ))}
        {result && <ImportResultView result={result} />}
      </div>
    </section>
  );
}

function ImportResultView({ result }: { result: ImportResult }) {
  const success = result.outcome === 'success';
  const rowErrors = result.errors.filter((e) => e.row !== null);
  const fileErrors = result.errors.filter((e) => e.row === null);

  return (
    <div className={`alert ${success ? 'success' : 'error'}`}>
      <strong>
        {success ? 'Import succeeded' : 'File rejected — nothing was imported'}
        {result.file_name ? ` (${result.file_name})` : ''}
      </strong>
      <p>{result.message}</p>

      {result.total_rows !== null && (
        <dl className="stats">
          <Stat label="Rows in file" value={result.total_rows} />
          {success ? (
            <>
              <Stat label="Inserted" value={result.inserted_rows} />
              <Stat label="Skipped (duplicates)" value={result.skipped_rows} />
            </>
          ) : (
            <>
              <Stat label="Invalid rows" value={result.invalid_rows} />
              <Stat label="Valid rows not imported" value={result.aborted_rows} />
            </>
          )}
        </dl>
      )}

      {fileErrors.map((e, i) => (
        <p key={i}>{e.messages.join(' ')}</p>
      ))}

      {rowErrors.length > 0 && (
        <>
          <p>Fix these rows and upload the file again. Row numbers count the header as row 1.</p>
          <div className="table-wrap">
            <table>
              <thead>
                <tr>
                  <th className="num">Row</th>
                  <th>Trade ID</th>
                  <th>Problem</th>
                </tr>
              </thead>
              <tbody>
                {rowErrors.map((e) => (
                  <tr key={e.row}>
                    <td className="num">{e.row}</td>
                    <td>{e.trade_id ?? '—'}</td>
                    <td>
                      <ul className="plain">
                        {e.messages.map((m, i) => (
                          <li key={i}>{m}</li>
                        ))}
                      </ul>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}
    </div>
  );
}

function Stat({ label, value }: { label: string; value: number | null }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd>{value ?? '—'}</dd>
    </div>
  );
}
