/**
 * A tiny in-process event bus. It exists so `events.ts` can deliver `doc-changed`, `open-file` and
 * friends in mock mode without statically importing the mock adapter — the adapter and its fixtures
 * must not land in the production bundle (they are loaded on demand by `api.ts`).
 */
export type BusListener = (payload: unknown) => void;

const listeners = new Map<string, Set<BusListener>>();

export const appBus = {
  on(name: string, fn: BusListener): () => void {
    const set = listeners.get(name) ?? new Set<BusListener>();
    listeners.set(name, set);
    set.add(fn);
    return () => {
      set.delete(fn);
    };
  },
  emit(name: string, payload: unknown): void {
    for (const fn of listeners.get(name) ?? []) fn(payload);
  },
  clear(): void {
    listeners.clear();
  },
};
