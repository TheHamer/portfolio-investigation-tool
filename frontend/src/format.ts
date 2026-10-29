// Display formatting. Values are decimal strings from the API and are
// formatted with string operations only, so precision is never lost.

/** Inserts thousands separators: "-1234567.891" -> "-1,234,567.891". */
export function groupDigits(value: string): string {
  const negative = value.startsWith('-');
  const unsigned = negative ? value.slice(1) : value;
  const [whole, fraction] = unsigned.split('.');
  const grouped = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
  return `${negative ? '-' : ''}${grouped}${fraction !== undefined ? `.${fraction}` : ''}`;
}

/** "1234.50" -> "£1,234.50"; "-12.30" -> "-£12.30". The backend has already
 * rounded the value half-up to 2 decimal places. */
export function formatGbp(value: string | null | undefined): string {
  if (value === null || value === undefined) return '—';
  const negative = value.startsWith('-');
  return `${negative ? '-' : ''}£${groupDigits(negative ? value.slice(1) : value)}`;
}

export function formatQuantity(value: number | null | undefined): string {
  if (value === null || value === undefined) return '—';
  return value.toLocaleString('en-GB');
}

/** CSS class for colouring a signed amount. */
export function signClass(value: string | null | undefined): string {
  if (!value || /^-?0(\.0+)?$/.test(value)) return '';
  return value.startsWith('-') ? 'negative' : 'positive';
}

/** "2026-01-22T08:52:12Z" -> "2026-01-22 08:52:12". */
export function formatTimestamp(value: string): string {
  return value.replace('T', ' ').replace('Z', '');
}
