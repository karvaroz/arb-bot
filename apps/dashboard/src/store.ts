import { create } from "zustand";

// Server state (pools/metrics/opportunities) is TanStack Query's job — pull,
// cached, revalidated. This store is for the one thing Query doesn't fit:
// the WS `Event` stream, which is push-driven and unbounded, not a
// request/response the cache can key on.
export type LiveEvent = { receivedAt: number; raw: unknown };

interface LiveState {
  connected: boolean;
  events: LiveEvent[];
  setConnected: (connected: boolean) => void;
  pushEvent: (raw: unknown) => void;
}

const MAX_EVENTS = 200;

export const useLiveStore = create<LiveState>((set) => ({
  connected: false,
  events: [],
  setConnected: (connected) => set({ connected }),
  pushEvent: (raw) =>
    set((state) => ({
      events: [{ receivedAt: Date.now(), raw }, ...state.events].slice(0, MAX_EVENTS),
    })),
}));
