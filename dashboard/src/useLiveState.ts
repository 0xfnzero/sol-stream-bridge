import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError, request, type Metrics, type Snapshot } from "./api";

export interface Sample {
  at: number;
  inbound: number;
  delivered: number;
}
export function rateSample(
  previous: { at: number; metrics: Metrics },
  next: Metrics,
  at: number,
): Sample | undefined {
  const seconds = (at - previous.at) / 1000;
  if (
    seconds <= 0.25 ||
    next.updates < previous.metrics.updates ||
    next.delivered < previous.metrics.delivered
  )
    return;
  return {
    at,
    inbound: (next.updates - previous.metrics.updates) / seconds,
    delivered: (next.delivered - previous.metrics.delivered) / seconds,
  };
}

export function useLiveState(
  token: string,
  initial: Snapshot,
  onExpired: () => void,
) {
  const [data, setData] = useState(initial);
  const [online, setOnline] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [lastSync, setLastSync] = useState(new Date());
  const [samples, setSamples] = useState<Sample[]>([]);
  const previous = useRef({ at: Date.now(), metrics: initial.metrics });
  const active = useRef<
    | { controller: AbortController; promise: Promise<Snapshot | undefined> }
    | undefined
  >(undefined);
  const alive = useRef(true);
  const expired = useRef(onExpired);
  expired.current = onExpired;
  const refresh = useCallback(
    (afterMutation = false) => {
      // Polls/manual refreshes share an in-flight read. A successful write needs a new read;
      // subsequent polls join that read instead of aborting the caller waiting for it.
      if (active.current && !afterMutation) return active.current.promise;
      active.current?.controller.abort();
      const controller = new AbortController();
      setRefreshing(true);
      const promise = (async () => {
        try {
          const next = await request<Snapshot>(
            token,
            "/api/state",
            "GET",
            undefined,
            controller.signal,
          );
          if (!alive.current || controller.signal.aborted) return;
          const at = Date.now();
          const sample = rateSample(previous.current, next.metrics, at);
          // Capture values before React evaluates the updater; never read a mutable ref inside it.
          if (sample) setSamples((old) => [...old, sample].slice(-30));
          previous.current = { at, metrics: next.metrics };
          setData(next);
          setOnline(true);
          setLastSync(new Date(at));
          return next;
        } catch (error) {
          if (!alive.current || controller.signal.aborted) return;
          if (error instanceof ApiError && error.status === 401)
            expired.current();
          else setOnline(false);
        } finally {
          if (active.current?.controller === controller) {
            active.current = undefined;
            if (alive.current) setRefreshing(false);
          }
        }
      })();
      active.current = { controller, promise };
      return promise;
    },
    [token],
  );
  useEffect(() => {
    alive.current = true;
    let timer: ReturnType<typeof setTimeout>;
    let generation = 0;
    const poll = async (current: number) => {
      if (document.hidden || !alive.current) return;
      await refresh();
      if (current === generation && alive.current && !document.hidden)
        timer = setTimeout(() => void poll(current), 2000);
    };
    const visible = () => {
      clearTimeout(timer);
      generation += 1;
      if (!document.hidden) void poll(generation);
    };
    timer = setTimeout(() => void poll(generation), 2000);
    document.addEventListener("visibilitychange", visible);
    return () => {
      alive.current = false;
      generation += 1;
      clearTimeout(timer);
      active.current?.controller.abort();
      document.removeEventListener("visibilitychange", visible);
    };
  }, [refresh]);
  return { data, online, refreshing, lastSync, samples, refresh };
}
