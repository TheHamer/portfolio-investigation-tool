// Minimal helpers for component tests that run in the jsdom environment
// (files starting with `// @vitest-environment jsdom`).

import { act, type ReactElement } from 'react';
import { createRoot } from 'react-dom/client';
import { vi } from 'vitest';

// Tells React that these tests wrap their updates in act().
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

/** Mounts `ui` in a fresh container and waits for its effects and any
 * already-resolved requests to settle. */
export async function render(ui: ReactElement) {
  const container = document.createElement('div');
  document.body.appendChild(container);
  const root = createRoot(container);

  await act(async () => {
    root.render(ui);
    await settle();
  });

  return {
    container,
    unmount: () => act(() => root.unmount()),
  };
}

/** Runs `action` inside act() and lets the promises it starts resolve. */
export async function interact(action: () => void) {
  await act(async () => {
    action();
    await settle();
  });
}

/** Every response `fetch` returns, in call order. */
export function mockFetch(...responses: { status: number; body: unknown }[]) {
  const fetch = vi.fn();
  for (const { status, body } of responses) {
    fetch.mockResolvedValueOnce(
      new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } }),
    );
  }
  vi.stubGlobal('fetch', fetch);
  return fetch;
}

const settle = () => new Promise((resolve) => setTimeout(resolve, 0));
