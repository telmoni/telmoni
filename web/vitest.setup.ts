import "@testing-library/jest-dom";
import { vi } from "vitest";

// ⚠ **Here, now that it keys more than the cookie seal.**
// `userChannel` derives an event channel name through an HMAC on this secret,
// so any suite that reaches a publish path needs it set — including suites
// that never mention sessions. `??=` so the handful that assign their own
// value to test sealing still win. Thirty-two bytes because `env.ts` refuses
// anything shorter.
process.env.AUTH_SECRET ??= "test-secret-that-is-32-chars-long!!";

if (typeof window !== "undefined") {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: vi.fn().mockImplementation((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addListener: vi.fn(),
      removeListener: vi.fn(),
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      dispatchEvent: vi.fn(),
    })),
  });

  global.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };

  if (typeof window.localStorage?.clear !== "function") {
    const store = new Map<string, string>();
    Object.defineProperty(window, "localStorage", {
      value: {
        getItem: (k: string) => store.get(k) ?? null,
        setItem: (k: string, v: string) => void store.set(k, String(v)),
        removeItem: (k: string) => void store.delete(k),
        clear: () => store.clear(),
        key: (i: number) => [...store.keys()][i] ?? null,
        get length() {
          return store.size;
        },
      },
      configurable: true,
    });
  }

  const observers: { cb: IntersectionObserverCallback }[] = [];
  (globalThis as Record<string, unknown>).intersectionObservers = observers;
  global.IntersectionObserver = class {
    constructor(cb: IntersectionObserverCallback) {
      observers.push({ cb });
    }
    observe() {}
    unobserve() {}
    disconnect() {}
    takeRecords() {
      return [];
    }
    root = null;
    rootMargin = "";
    thresholds = [];
  } as unknown as typeof IntersectionObserver;
}
