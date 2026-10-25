import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api, type AccountSummary, type Instance, type InstanceState, type MultiInstanceStatus } from "../../api";
import { qk, useInstances, useMultiStatus } from "../../data";
import { Switch } from "../../ui/controls";
import { ConfirmDialog } from "../../ui/Dialog";
import { Icon } from "../../ui/Icon";
import { toastError } from "../../ui/Toast";
import { accountName, Avatar, plural } from "../accounts/shared";

const STATES: Record<InstanceState, [label: string, color: string]> = {
  starting: ["Starting", "var(--dim)"],
  joining: ["Joining", "var(--blue)"],
  inGame: ["In game", "var(--accent-text)"],
  disconnected: ["Disconnected", "var(--orange)"],
  exited: ["Ended", "var(--dim)"],
};

export const isRunning = (i: Instance) => i.state !== "exited";

function uptime(from: number, to: number) {
  const m = Math.max(0, Math.floor((to - from) / 60_000));
  return m < 60 ? `${m}m` : `${Math.floor(m / 60)}h ${m % 60}m`;
}

/** Re-renders every `ms` so uptimes move. */
function useNow(ms: number) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(t);
  }, [ms]);
  return now;
}

/** The Launch view's right column: clients RoALT started, newest first. */
export function RunningPanel({ accounts }: { accounts: AccountSummary[] }) {
  const list = useInstances().data ?? [];
  const now = useNow(30_000);
  const [closing, setClosing] = useState<ReadonlySet<string>>(new Set());
  const [confirmAll, setConfirmAll] = useState(false);
  const [busyAll, setBusyAll] = useState(false);
  const byId = new Map(accounts.map((a) => [a.id, a]));
  const running = list.filter(isRunning);
  const rows = [...list].sort((a, b) => Number(isRunning(b)) - Number(isRunning(a)) || b.startedAt - a.startedAt);

  const close = (i: Instance) => {
    setClosing((s) => new Set(s).add(i.id));
    api.instanceClose(i.id, false).catch(toastError);
  };
  const closeAll = () => {
    setBusyAll(true);
    api
      .instancesCloseAll(false)
      .catch(toastError)
      .finally(() => {
        setBusyAll(false);
        setConfirmAll(false);
      });
  };

  return (
    <aside aria-label="Running clients" className="flex-[1_1_400px] min-w-0 max-w-[560px] flex flex-col bg-panel border-l border-line">
      <div className="flex items-center gap-2.5 px-5 pt-[18px] pb-2.5">
        <span className="label whitespace-nowrap">Running clients</span>
        <span className="mono text-xs text-dim">{running.length}</span>
        <span className="ml-auto flex gap-2">
          {running.length < list.length && (
            <button className="btn btn-sm" onClick={() => api.instancesClearEnded().catch(toastError)}>
              Clear ended
            </button>
          )}
          <button className="btn btn-sm" disabled={running.length === 0} onClick={() => setConfirmAll(true)}>
            Close all
          </button>
        </span>
      </div>
      {rows.length === 0 ? (
        <p className="px-5 py-2 text-dim">Clients you launch show here until they close.</p>
      ) : (
        <div className="scroll flex-1 min-h-0">
          <table className="mono w-full border-collapse text-xs">
            <thead>
              <tr className="h-7 text-dim text-left">
                <th className="pl-5 font-normal border-b border-line">Account</th>
                <th className="font-normal border-b border-line">State</th>
                <th className="font-normal border-b border-line">Job</th>
                <th className="font-normal border-b border-line">Up</th>
                <th className="w-12 border-b border-line">
                  <span className="sr-only">Close</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((i) => {
                const a = byId.get(i.accountId);
                const name = a ? accountName(a) : `Account ${i.accountId}`;
                const [label, color] = STATES[i.state];
                const live = isRunning(i);
                return (
                  <tr key={i.id} className={`enter h-10 border-b border-line ${live ? "" : "text-dim"}`} title={i.note ?? `PID ${i.pid}`}>
                    <td className="pl-5 pr-2 max-w-0 w-[40%]">
                      <span className="flex items-center gap-2 min-w-0 font-sans">
                        {a && <Avatar account={a} size={20} />}
                        <span className="truncate">{name}</span>
                      </span>
                    </td>
                    <td className="pr-2 whitespace-nowrap">
                      <span className="inline-flex items-center gap-1.5">
                        <span aria-hidden="true" className={`w-1.5 h-1.5 flex-none ${i.state === "starting" || i.state === "joining" ? "pulse" : ""}`} style={{ background: color }} />
                        {live ? label : i.note ?? label}
                      </span>
                    </td>
                    <td className="pr-2 text-dim" title={i.jobId ?? undefined}>{i.jobId ? i.jobId.slice(0, 8) : "—"}</td>
                    <td className="pr-2 whitespace-nowrap">{uptime(i.startedAt, i.endedAt ?? now)}</td>
                    <td>
                      {live && (
                        <button
                          className="wc w-9 h-8"
                          aria-label={`Close ${name}'s client`}
                          title="Close this client"
                          disabled={closing.has(i.id)}
                          onClick={() => close(i)}
                        >
                          <Icon name="x" size={10} />
                        </button>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
      <ConfirmDialog
        open={confirmAll}
        onClose={() => !busyAll && setConfirmAll(false)}
        onConfirm={closeAll}
        busy={busyAll}
        title="Close all clients?"
        text={<>{plural(running.length, "Roblox client")} RoALT started will close. Unsaved progress in those games is lost.</>}
        confirmLabel="Close all"
      />
    </aside>
  );
}

/** Sidebar switch for multi-instance, with a way out when Roblox already runs. */
export function MultiInstanceSwitch() {
  const qc = useQueryClient();
  const status = useMultiStatus().data;
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const apply = (s: MultiInstanceStatus) => qc.setQueryData(qk.multi, s);

  const toggle = (enabled: boolean) => {
    api.multiInstanceSet(enabled).then((s) => {
      apply(s);
      qc.invalidateQueries({ queryKey: qk.settings });
    }, toastError);
  };
  const closeRoblox = () => {
    setBusy(true);
    api
      .robloxCloseAll(false)
      .then(apply, toastError)
      .finally(() => {
        setBusy(false);
        setConfirm(false);
      });
  };

  if (!status) return null;
  const blocked = status.enabled && !status.active;
  return (
    <>
      <div className="flex items-center justify-between gap-2">
        <span className="font-sans text-[13px] text-text">Multi-instance</span>
        <Switch label="Multi-instance" checked={status.enabled} onChange={toggle} />
      </div>
      {blocked && (
        <div className="enter flex flex-col items-start gap-2 font-sans text-xs">
          <span className="alert">{status.message}</span>
          {status.blockedBy.length > 0 && (
            <button className="btn btn-sm" onClick={() => setConfirm(true)}>
              Close Roblox…
            </button>
          )}
        </div>
      )}
      <ConfirmDialog
        open={confirm}
        onClose={() => !busy && setConfirm(false)}
        onConfirm={closeRoblox}
        busy={busy}
        title="Close Roblox?"
        text={<>Every running Roblox client closes ({status.blockedBy.length} process{status.blockedBy.length === 1 ? "" : "es"}), including ones RoALT didn't start. Unsaved progress in those games is lost.</>}
        confirmLabel="Close Roblox"
      />
    </>
  );
}
