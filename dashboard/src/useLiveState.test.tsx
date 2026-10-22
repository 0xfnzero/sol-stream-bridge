import { act, cleanup, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { ApiError, request, type Snapshot } from "./api";
import { rateSample, useLiveState } from "./useLiveState";

vi.mock("./api", async (original) => ({
  ...(await original<typeof import("./api")>()),
  request: vi.fn(),
}));
const read = vi.mocked(request);
// The hook consumes metrics and preserves the remaining snapshot as an opaque value.
const initial = {
  revision: "0",
  metrics: { updates: 10, delivered: 20 },
} as Snapshot;
function pending() {
  let resolve!: (value: Snapshot) => void;
  const promise = new Promise<Snapshot>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}
beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(10000);
  vi.spyOn(document, "hidden", "get").mockReturnValue(false);
  read.mockResolvedValue(initial);
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
  vi.clearAllMocks();
});
it("deduplicates polls and ignores stale responses after a successful write", async () => {
  const first = pending(),
    second = pending();
  read.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
  const { result } = renderHook(() => useLiveState("token", initial, vi.fn()));
  let old!: Promise<Snapshot | undefined>,
    fresh!: Promise<Snapshot | undefined>;
  act(() => {
    old = result.current.refresh();
  });
  act(() => {
    expect(result.current.refresh()).toBe(old);
  });
  act(() => {
    fresh = result.current.refresh(true);
  });
  expect(read.mock.calls[0][4]?.aborted).toBe(true);
  act(() => {
    expect(result.current.refresh()).toBe(fresh);
  });
  const changed = { ...initial, revision: "1" };
  await act(async () => {
    second.resolve(changed);
    expect(await fresh).toBe(changed);
  });
  await act(async () => {
    first.resolve(initial);
    expect(await old).toBeUndefined();
  });
  expect(result.current.data.revision).toBe("1");
  expect(result.current.refreshing).toBe(false);
  expect(read).toHaveBeenCalledTimes(2);
});
it("pauses background polling and resumes immediately without duplicate timers", async () => {
  const { result } = renderHook(() => useLiveState("token", initial, vi.fn()));
  await act(async () => {
    await vi.advanceTimersByTimeAsync(2000);
  });
  expect(read).toHaveBeenCalledTimes(1);
  vi.spyOn(document, "hidden", "get").mockReturnValue(true);
  act(() => {
    document.dispatchEvent(new Event("visibilitychange"));
  });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(10000);
  });
  expect(read).toHaveBeenCalledTimes(1);
  vi.spyOn(document, "hidden", "get").mockReturnValue(false);
  await act(async () => {
    document.dispatchEvent(new Event("visibilitychange"));
    document.dispatchEvent(new Event("visibilitychange"));
  });
  expect(read).toHaveBeenCalledTimes(2);
  await act(async () => {
    await vi.advanceTimersByTimeAsync(2000);
  });
  expect(read).toHaveBeenCalledTimes(3);
  expect(result.current.online).toBe(true);
});
it("aborts on unmount and ignores late data", async () => {
  const response = pending();
  read.mockReturnValue(response.promise);
  const expired = vi.fn();
  const { result, unmount } = renderHook(() =>
    useLiveState("token", initial, expired),
  );
  let promise!: Promise<Snapshot | undefined>;
  act(() => {
    promise = result.current.refresh();
  });
  unmount();
  expect(read.mock.calls[0][4]?.aborted).toBe(true);
  response.resolve({ ...initial, revision: "late" });
  expect(await promise).toBeUndefined();
  expect(expired).not.toHaveBeenCalled();
});
it("reports session expiry and recovers from a transient connection failure", async () => {
  const expired = vi.fn();
  const { result } = renderHook(() => useLiveState("token", initial, expired));
  read.mockRejectedValueOnce(new TypeError("offline"));
  await act(async () => {
    await result.current.refresh();
  });
  expect(result.current.online).toBe(false);
  await act(async () => {
    await result.current.refresh();
  });
  expect(result.current.online).toBe(true);
  read.mockRejectedValueOnce(new ApiError(401, "expired"));
  await act(async () => {
    await result.current.refresh();
  });
  expect(expired).toHaveBeenCalledTimes(1);
});
it("calculates rates from elapsed time and rejects clock/counter resets", () => {
  const previous = { at: 1000, metrics: initial.metrics };
  const metrics = { ...initial.metrics, updates: 30, delivered: 80 };
  expect(rateSample(previous, metrics, 3000)).toEqual({
    at: 3000,
    inbound: 10,
    delivered: 30,
  });
  expect(rateSample(previous, metrics, 1000)).toBeUndefined();
  expect(rateSample(previous, metrics, 900)).toBeUndefined();
  expect(
    rateSample(previous, { ...metrics, updates: 0 }, 3000),
  ).toBeUndefined();
  expect(
    rateSample(previous, { ...metrics, delivered: 0 }, 3000),
  ).toBeUndefined();
});
