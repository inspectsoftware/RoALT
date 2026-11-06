// Sessions' queries: their own keys (not in data.ts's qk, which other streams share).
import { useEffect, useState, type RefObject } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { sessionsApi, type SessionConfig, type SessionView } from "../../api";
import { toastError } from "../../ui/Toast";

export const sk = {
  all: ["sessions"] as const,
  list: ["sessions", "list"] as const,
  health: ["sessions", "health"] as const,
  localMacros: ["sessions", "localMacros"] as const,
  state: (name: string) => ["sessions", "state", name] as const,
  macros: (name: string) => ["sessions", "macros", name] as const,
};

/** Sessions' Windows and control states move on their own: asked again every 3 s while the page is open. */
export const useSessions = () => useQuery({ queryKey: sk.list, queryFn: sessionsApi.sessionsList, refetchInterval: 3_000 });
export const useHealth = () => useQuery({ queryKey: sk.health, queryFn: sessionsApi.sessionsHealth });
export const useLocalMacros = (enabled: boolean) => useQuery({ queryKey: sk.localMacros, queryFn: sessionsApi.macrosList, enabled });

/** The clients of a session whose control API answers, every 2 s (paused while the window is hidden). */
export const useRemoteState = (name: string, enabled: boolean) =>
  useQuery({ queryKey: sk.state(name), queryFn: () => sessionsApi.remoteState(name), enabled, refetchInterval: 2_000 });
export const useRemoteMacros = (name: string, enabled: boolean) =>
  useQuery({ queryKey: sk.macros(name), queryFn: () => sessionsApi.remoteMacros(name), enabled });

export const controlOk = (s: SessionView) => s.control.ok && s.wts !== "none";

/** One action at a time per key: `busy` names the running one; a failure toasts; afterwards the
 * sessions (and what `also` names) are asked again. Settles true when it worked. */
export function useAct() {
  const qc = useQueryClient();
  const [busy, setBusy] = useState<string | null>(null);
  const run = async (key: string, f: () => Promise<unknown>, also: readonly (readonly string[])[] = []) => {
    setBusy(key);
    try {
      await f();
      return true;
    } catch (e) {
      toastError(e);
      return false;
    } finally {
      setBusy((b) => (b === key ? null : b));
      for (const queryKey of [sk.list, ...also]) void qc.invalidateQueries({ queryKey });
    }
  };
  return { busy, run };
}

/** The next free session slot: `altdock-n`, 127.0.0.(n+1), port 47610+n (docs/ALTDOCK.md, Paths). */
export function nextSlot(list: SessionConfig[]) {
  let n = 1;
  while (list.some((c) => c.windowsUser === `altdock-${n}` || c.port === 47610 + n)) n++;
  return { windowsUser: `altdock-${n}`, address: `127.0.0.${n + 1}`, port: 47610 + n };
}

const STATUSES = ["live", "minimized", "noWindow", "pending", "unsupported"] as const;
export type RemotePicture = (typeof STATUSES)[number];
/** About one picture a second. */
const EVERY_MS = 1000;

/** Draws a session client's window on `canvas` about once a second, only while AltDock's window is
 * visible and the card is on screen; one request in flight at a time. */
export function useRemoteThumbnail(session: string, id: string, canvas: RefObject<HTMLCanvasElement | null>) {
  const [status, setStatus] = useState<RemotePicture | null>(null);
  const [painted, setPainted] = useState(false);
  useEffect(() => {
    let stop = false;
    let timer: number | undefined;
    let inView = true;
    const seen = new IntersectionObserver((entries) => void (inView = entries[entries.length - 1].isIntersecting));
    if (canvas.current) seen.observe(canvas.current);
    const tick = async () => {
      const started = performance.now();
      const el = canvas.current;
      if (el && inView && document.visibilityState === "visible") {
        try {
          const buf = await sessionsApi.remoteThumbnail(session, id, Math.min(960, Math.max(320, Math.round(el.clientWidth * devicePixelRatio))));
          if (stop) return;
          const head = new DataView(buf);
          const [w, h, code] = [head.getUint32(0, true), head.getUint32(4, true), head.getUint32(8, true)];
          if (buf.byteLength > 24 && w && h) {
            if (el.width !== w) el.width = w;
            if (el.height !== h) el.height = h;
            el.getContext("2d")?.putImageData(new ImageData(new Uint8ClampedArray(buf, 24, w * h * 4), w, h), 0, 0);
            setPainted(true);
          }
          setStatus(STATUSES[code] ?? null);
        } catch {
          // Closed, or the session went away: the card goes with the next state.
        }
      }
      if (!stop) timer = window.setTimeout(tick, Math.max(0, EVERY_MS - (performance.now() - started)));
    };
    void tick();
    return () => {
      stop = true;
      clearTimeout(timer);
      seen.disconnect();
    };
  }, [session, id, canvas]);
  return { status, painted };
}
