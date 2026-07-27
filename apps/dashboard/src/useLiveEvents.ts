import { useEffect } from "react";
import { wsUrl } from "./api";
import { useLiveStore } from "./store";

// One WS connection per mounted dashboard, reconnects on drop — the pipeline
// process outlives any single browser tab, so a dropped connection (backend
// restart, network blip) is expected, not exceptional.
export function useLiveEvents() {
  const setConnected = useLiveStore((s) => s.setConnected);
  const pushEvent = useLiveStore((s) => s.pushEvent);

  useEffect(() => {
    let socket: WebSocket | null = null;
    let retryTimer: ReturnType<typeof setTimeout> | null = null;
    let cancelled = false;

    const connect = () => {
      if (cancelled) return;
      socket = new WebSocket(wsUrl());
      socket.onopen = () => setConnected(true);
      socket.onclose = () => {
        setConnected(false);
        if (!cancelled) retryTimer = setTimeout(connect, 2000);
      };
      socket.onerror = () => socket?.close();
      socket.onmessage = (msg) => {
        try {
          pushEvent(JSON.parse(msg.data));
        } catch {
          // ignore malformed frames
        }
      };
    };
    connect();

    return () => {
      cancelled = true;
      if (retryTimer) clearTimeout(retryTimer);
      socket?.close();
    };
  }, [setConnected, pushEvent]);
}
