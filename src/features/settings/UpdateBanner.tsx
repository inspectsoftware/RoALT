import { useState, useSyncExternalStore } from "react";
import { useQuery } from "@tanstack/react-query";
import { api } from "../../api";
import { qk, useInstances } from "../../data";
import { ConfirmDialog } from "../../ui/Dialog";
import { Icon } from "../../ui/Icon";
import { toastError } from "../../ui/Toast";
import { plural } from "../accounts/shared";
import { isRunning } from "../launch/Running";

/** The version Later hid: not offered again until RoALT restarts, or Check now finds it. */
let hidden: string | null = null;
const subs = new Set<() => void>();
const hide = (version: string | null) => {
  hidden = version;
  subs.forEach((f) => f());
};
const subscribe = (f: () => void) => {
  subs.add(f);
  return () => void subs.delete(f);
};
export const unhideUpdate = () => hide(null);

/** "RoALT x.y.z is ready" under the title bar while a newer release is known (self_update). */
export function UpdateBanner() {
  // The start-up check can finish before the app://update listener exists: ask the cached
  // result again (no network) until an update is known.
  const version = useQuery({ queryKey: qk.appUpdate, queryFn: api.selfUpdateStatus, refetchInterval: (q) => (q.state.data?.available ? false : 5_000) }).data?.available;
  const running = useInstances().data?.filter(isRunning).length ?? 0;
  const later = useSyncExternalStore(subscribe, () => hidden);
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  if (!version || version === later) return null;

  // Resolves just before RoALT quits for the installer, which starts it again.
  async function update() {
    setConfirm(false);
    setBusy(true);
    try {
      await api.selfUpdateInstall();
    } catch (e) {
      toastError(e);
      setBusy(false);
    }
  }

  return (
    <div role="status" className="enter flex-none flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-2 bg-panel border-b border-line">
      <Icon name="download" />
      <span className="flex-1 min-w-0">
        RoALT <span className="mono">{version}</span> is ready.
        {busy && <span className="text-dim"> Downloading and checking its signature…</span>}
      </span>
      <button className="btn btn-sm btn-primary" disabled={busy} onClick={() => (running ? setConfirm(true) : void update())}>
        Update now
      </button>
      <button className="btn btn-sm" disabled={busy} onClick={() => hide(version)}>
        Later
      </button>
      <ConfirmDialog
        open={confirm}
        onClose={() => setConfirm(false)}
        onConfirm={() => void update()}
        title={`Update RoALT to ${version}?`}
        text={`RoALT closes while it updates and starts again by itself. ${plural(running, "Roblox client")} ${running === 1 ? "keeps" : "keep"} running, but the restarted RoALT doesn't track ${running === 1 ? "it" : "them"}: ${running === 1 ? "it leaves" : "they leave"} Running clients, and auto-rejoin, crash restarts, anti-AFK and memory rules stop for ${running === 1 ? "it" : "them"}.`}
        confirmLabel="Update now"
        safe
      />
    </div>
  );
}
