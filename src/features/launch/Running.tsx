import { Fragment, useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api, type AccountSummary, type Crash, type CrashKind, type Instance, type InstanceState, type Limits, type MultiInstanceStatus } from "../../api";
import { qk, useBulk, useCrashes, useInstances, useMultiStatus, useSettings } from "../../data";
import { Segmented, Switch } from "../../ui/controls";
import { ConfirmDialog } from "../../ui/Dialog";
import { Divider, usePanelWidth } from "../../ui/Resizer";
import { Icon } from "../../ui/Icon";
import { toast, toastError } from "../../ui/Toast";
import { accountName, Avatar, plural } from "../accounts/shared";
import { BatchSection } from "./Batch";
import { launchBlocked, launched, launchFailed } from "./LaunchView";

export const STATES: Record<InstanceState, [label: string, color: string]> = {
  starting: ["Starting", "var(--dim)"],
  joining: ["Joining", "var(--blue)"],
  inGame: ["In game", "var(--accent-text)"],
  disconnected: ["Disconnected", "var(--orange)"],
  exited: ["Ended", "var(--dim)"],
};

export const isRunning = (i: Instance) => i.state !== "exited";

export function uptime(from: number, to: number) {
  const m = Math.max(0, Math.floor((to - from) / 60_000));
  return m < 60 ? `${m}m` : `${Math.floor(m / 60)}h ${m % 60}m`;
}

/** Re-renders every `ms` so uptimes move. */
export function useNow(ms: number) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(t);
  }, [ms]);
  return now;
}

const CAPS = [0, 50, 25, 10, 5];
/** -1: the default from Settings; 0: never. */
const TRIMS = [-1, 0, 5, 15, 30, 60];
const limited = (l: Limits) => l.cpuCap !== null || l.eco || l.trimEveryMin !== null || l.affinity !== null;
/** Logical processors a mask can name (JS numbers hold 53 bits exactly). */
const CORES = Math.min(53, navigator.hardwareConcurrency || 1);
// Not bit operators: those cut numbers to 32 bits.
const hasCore = (mask: number, c: number) => Math.floor(mask / 2 ** c) % 2 === 1;
const allCores = 2 ** CORES - 1;

/** One client's utility panel, under its row: CPU cap, efficiency mode, trimming, cores and anti-AFK. */
function LimitsRow({ inst }: { inst: Instance }) {
  const set = (patch: Partial<Limits>) => api.instanceSetLimits(inst.id, { ...inst.limits, ...patch }).catch(toastError);
  const afk = useSettings().data?.antiAfk;
  const [trimming, setTrimming] = useState(false);
  const mask = inst.limits.affinity ?? allCores;
  const toggleCore = (c: number) => {
    const next = hasCore(mask, c) ? mask - 2 ** c : mask + 2 ** c;
    if (next === 0) return toastError({ kind: "invalid", message: "Leave at least one CPU core." });
    void set({ affinity: next === allCores ? null : next });
  };
  const trimNow = () => {
    setTrimming(true);
    api
      .instanceTrim(inst.id)
      .then(() => toast("Trimmed its memory. Windows pages it back in as the client needs it."), toastError)
      .finally(() => setTrimming(false));
  };
  return (
    <tr className="border-b border-line">
      <td colSpan={5} className="px-5 py-3 font-sans">
        <div className="flex flex-col gap-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="text-[13px] text-text">CPU limit</span>
            <Segmented
              mono
              label="CPU limit"
              value={inst.limits.cpuCap ?? 0}
              options={CAPS.map((c) => ({ value: c, label: c ? `${c}%` : "Off" }))}
              onChange={(c) => set({ cpuCap: c || null })}
            />
          </div>
          <div className="flex items-center justify-between gap-2">
            <span className="text-[13px] text-text">Efficiency mode</span>
            <Switch label="Efficiency mode" checked={inst.limits.eco} onChange={(eco) => set({ eco })} />
          </div>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="text-[13px] text-text">Trim memory</span>
            <span className="flex flex-wrap items-center gap-2">
              <Segmented
                mono
                label="Trim memory every"
                value={inst.limits.trimEveryMin ?? -1}
                options={TRIMS.map((m) => ({ value: m, label: m < 0 ? "Default" : m ? `${m}m` : "Off" }))}
                onChange={(m) => set({ trimEveryMin: m < 0 ? null : m })}
              />
              <button className="btn btn-sm" disabled={trimming} onClick={trimNow}>
                {trimming ? "Trimming…" : "Trim now"}
              </button>
            </span>
          </div>
          <fieldset className="flex flex-col gap-1.5">
            <legend className="flex w-full items-center justify-between gap-2 text-[13px] text-text">
              CPU cores
              {inst.limits.affinity !== null && (
                <button type="button" className="btn btn-sm" onClick={() => set({ affinity: null })}>All cores</button>
              )}
            </legend>
            <div className="flex flex-wrap gap-1 mt-1.5">
              {Array.from({ length: CORES }, (_, c) => (
                <label key={c} className="mono flex items-center gap-1 text-xs text-dim w-11">
                  <input type="checkbox" checked={hasCore(mask, c)} onChange={() => toggleCore(c)} />
                  {c}
                </label>
              ))}
            </div>
          </fieldset>
          <div className="flex items-center justify-between gap-2">
            <span className="flex flex-col">
              <span className="text-[13px] text-text">Anti-AFK</span>
              <span className="mono text-xs text-dim">
                {!afk?.enabled ? "Off in Settings" : inst.afkAt !== null ? `Last key press ${uptime(inst.afkAt, Date.now())} ago` : "No key press yet"}
              </span>
            </span>
            <Switch label="Anti-AFK for this client" checked={inst.antiAfk} onChange={(on) => api.instanceSetAntiAfk(inst.id, on).catch(toastError)} />
          </div>
          <p className="text-xs text-dim">
            The limit is a share of all CPU cores; its frame rate drops with it. Roblox has no per-client FPS cap RoALT
            can set without modifying the client. Efficiency mode lowers its priority, as Task Manager does. Trimming asks
            Windows to page its memory out; it comes back as used. Limits set here win over the adaptive manager's.
            Anti-AFK briefly brings this client to the front for a Space press while you're away; it skips a minimized client.
          </p>
        </div>
      </td>
    </tr>
  );
}

const KINDS: Record<CrashKind, string> = {
  crash: "Crashed",
  disconnect: "Disconnected by Roblox",
  hang: "Stopped responding",
  mem: "Restarted: used too much memory",
};

/** What RoALT recorded about an ended client: how it ended, what its log said, its log. */
function CrashRow({ crash: c }: { crash: Crash }) {
  return (
    <tr className="border-b border-line">
      <td colSpan={5} className="px-5 py-3 font-sans">
        <div className="flex flex-col gap-2">
          <div className="flex flex-wrap items-baseline justify-between gap-2">
            <span className="text-[13px] text-text">{KINDS[c.kind]}</span>
            <span className="mono text-xs text-dim">after {uptime(0, c.uptimeMs)}{c.placeId !== null ? ` · place ${c.placeId}` : ""}</span>
          </div>
          {c.reason && <p className="mono text-xs text-dim break-words selectable">{c.reason}</p>}
          {c.hasLog && (
            <button className="btn btn-sm self-start" onClick={() => api.crashOpenLog(c.id).catch(toastError)}>
              Open log
              <Icon name="external" size={12} />
            </button>
          )}
        </div>
      </td>
    </tr>
  );
}

/** The Launch view's right column: the bulk launch, then clients RoALT started, newest first. */
export function RunningPanel({ accounts, onPick }: { accounts: AccountSummary[]; onPick: (ids: number[]) => void }) {
  const width = usePanelWidth("running", 400, 300, 720);
  const list = useInstances().data ?? [];
  const now = useNow(30_000);
  const [closing, setClosing] = useState<ReadonlySet<string>>(new Set());
  const [confirmAll, setConfirmAll] = useState(false);
  const [busyAll, setBusyAll] = useState(false);
  const [tuning, setTuning] = useState<string | null>(null);
  const [details, setDetails] = useState<string | null>(null);
  const crashes = new Map(useCrashes().data?.map((c) => [c.instanceId, c]));
  const [relaunching, setRelaunching] = useState<string | null>(null);
  const multi = useMultiStatus().data;
  const batch = useBulk().data;
  const byId = new Map(accounts.map((a) => [a.id, a]));
  const running = list.filter(isRunning);
  const rows = [...list].sort((a, b) => Number(isRunning(b)) - Number(isRunning(a)) || b.startedAt - a.startedAt);

  // Keyed by pid too: a close that reached only a bootstrapper ends with the client adopted
  // under a new pid, which can then be closed again.
  const closeKey = (i: Instance) => `${i.id}:${i.pid}`;
  const close = (i: Instance) => {
    const key = closeKey(i);
    setClosing((s) => new Set(s).add(key));
    api.instanceClose(i.id, false).catch((e) => {
      setClosing((s) => {
        const next = new Set(s);
        next.delete(key);
        return next;
      });
      toastError(e);
    });
  };
  /** Same account, same target, for an ended row. */
  const relaunch = (i: Instance, a: AccountSummary) => {
    setRelaunching(i.id);
    api
      .launch(a.id, i.target)
      .then((info) => launched(a, info), launchFailed)
      .finally(() => setRelaunching(null));
  };
  // Starting next to running clients needs multi-instance, as a bulk launch does.
  const relaunchBlocked = launchBlocked(running.length > 0, multi, batch) ?? (relaunching ? "Launching…" : null);
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
    <>
    <Divider panel={width} side="right" label="Running clients width" />
    <aside aria-label="Running clients" style={{ width: width.width }} className="@container flex-none min-w-0 flex flex-col bg-panel border-l border-line">
      <BatchSection accounts={accounts} onPick={onPick} />
      <div className="flex flex-wrap items-center gap-2.5 px-5 pt-[18px] pb-2.5">
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
                <th className="font-normal border-b border-line hidden @sm:table-cell">Job</th>
                <th className="font-normal border-b border-line">Up</th>
                <th className="w-20 border-b border-line">
                  <span className="sr-only">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((i) => {
                const a = byId.get(i.accountId);
                const name = a ? accountName(a) : `Account ${i.accountId}`;
                const [label, color] = STATES[i.state];
                const live = isRunning(i);
                const crash = live ? undefined : crashes.get(i.id);
                return (
                  <Fragment key={i.id}>
                  <tr className={`enter h-10 border-b border-line ${live ? "" : "text-dim"}`} title={`PID ${i.pid}${i.afkAt !== null ? ` · anti-AFK key press ${uptime(i.afkAt, now)} ago` : ""}`}>
                    <td className="pl-5 pr-2 max-w-0 w-[40%]">
                      <span className="flex items-center gap-2 min-w-0 font-sans">
                        {a && <Avatar account={a} size={20} />}
                        <span className="truncate">{name}</span>
                        {i.rejoins > 0 && (
                          <span className="mono flex-none text-dim" title={`Rejoined ${plural(i.rejoins, "time")}`}>
                            <span aria-hidden="true">↻{i.rejoins}</span>
                            <span className="sr-only">, rejoined {plural(i.rejoins, "time")}</span>
                          </span>
                        )}
                      </span>
                    </td>
                    <td className="pr-2 py-1.5">
                      <span className="flex items-baseline gap-1.5">
                        <span aria-hidden="true" className={`w-1.5 h-1.5 flex-none ${i.state === "starting" || i.state === "joining" ? "pulse" : ""}`} style={{ background: color }} />
                        <span>
                          {live ? (closing.has(closeKey(i)) ? "Closing…" : label) : i.note ?? label}
                          {live && i.note && <span className="text-orange"> · {i.note}</span>}
                        </span>
                      </span>
                    </td>
                    <td className="pr-2 text-dim hidden @sm:table-cell" title={i.jobId ?? undefined}>{i.jobId ? i.jobId.slice(0, 8) : "—"}</td>
                    <td className="pr-2 whitespace-nowrap">{uptime(i.startedAt, i.endedAt ?? now)}</td>
                    <td>
                      {live && (
                        <span className="flex">
                        <button
                          className={`wc w-9 h-8 ${limited(i.limits) ? "text-accent-text" : ""}`}
                          aria-label={`Limits for ${name}'s client`}
                          aria-expanded={tuning === i.id}
                          title={limited(i.limits) ? `Limited${i.limits.cpuCap ? ` to ${i.limits.cpuCap}% CPU` : ""}${i.limits.eco ? ", efficiency mode" : ""}${i.limits.trimEveryMin !== null ? ", trim" : ""}${i.limits.affinity !== null ? ", cores" : ""}` : "CPU limit, efficiency mode, trimming and cores"}
                          onClick={() => setTuning((t) => (t === i.id ? null : i.id))}
                        >
                          <Icon name="sliders" size={12} />
                        </button>
                        <button
                          className="wc w-9 h-8"
                          aria-label={`Close ${name}'s client`}
                          title="Close this client"
                          disabled={closing.has(closeKey(i))}
                          onClick={() => close(i)}
                        >
                          <Icon name="x" size={10} />
                        </button>
                        </span>
                      )}
                      {!live && (
                        <span className="flex">
                        {crash && (
                          <button
                            className="wc w-9 h-8"
                            aria-label={`How ${name}'s client ended`}
                            aria-expanded={details === i.id}
                            title="How it ended"
                            onClick={() => setDetails((d) => (d === i.id ? null : i.id))}
                          >
                            <Icon name="chevron" size={12} className={details === i.id ? "rotate-180" : ""} />
                          </button>
                        )}
                        {a?.status === "valid" && !running.some((r) => r.accountId === a.id && r.state !== "disconnected") && (
                        <button
                          className="wc w-9 h-8"
                          aria-label={`Relaunch ${name}`}
                          title={relaunchBlocked ?? "Launch this account into the same game again"}
                          disabled={relaunchBlocked !== null}
                          onClick={() => relaunch(i, a)}
                        >
                          <Icon name="play" size={10} />
                        </button>
                        )}
                        </span>
                      )}
                    </td>
                  </tr>
                  {live && tuning === i.id && <LimitsRow inst={i} />}
                  {crash && details === i.id && <CrashRow crash={crash} />}
                  </Fragment>
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
    </>
  );
}

const HOLD_MS = 1000;

/** The panic button: held for a second (pointer, Space or Enter), it cancels the batch, stops
 * rejoins and restarts, and terminates every Roblox client at once, RoALT's or not. */
export function KillAllButton() {
  const qc = useQueryClient();
  const [held, setHeld] = useState(false);
  const [busy, setBusy] = useState(false);
  const timer = useRef<number | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  const fire = () => {
    setHeld(false);
    setBusy(true);
    api
      .panicKill()
      .then((st) => {
        qc.setQueryData(qk.multi, st);
        toast("Killed every Roblox client.");
      }, toastError)
      .finally(() => setBusy(false));
  };
  const start = () => {
    if (busy || held) return;
    setHeld(true);
    timer.current = window.setTimeout(fire, HOLD_MS);
  };
  const stop = () => {
    clearTimeout(timer.current);
    setHeld(false);
  };
  const key = (e: { key: string }) => e.key === " " || e.key === "Enter";
  return (
    <button
      type="button"
      className="btn btn-sm btn-danger relative overflow-hidden w-full"
      disabled={busy}
      title="Hold for a second: ends every Roblox client at once, without saving"
      onPointerDown={start}
      onPointerUp={stop}
      onPointerLeave={stop}
      onKeyDown={(e) => {
        if (!key(e)) return;
        e.preventDefault();
        if (!e.repeat) start();
      }}
      onKeyUp={(e) => key(e) && stop()}
      onBlur={stop}
    >
      <span
        aria-hidden="true"
        className="absolute inset-y-0 left-0"
        style={{
          width: held ? "100%" : "0%",
          background: "color-mix(in srgb, var(--orange) 35%, transparent)",
          transition: held ? `width ${HOLD_MS}ms linear` : "none",
        }}
      />
      <span className="relative">{busy ? "Killing…" : held ? "Keep holding…" : "Hold to kill all Roblox"}</span>
    </button>
  );
}

/** Sidebar switch for multi-instance, with a way out when Roblox already runs. */
const NOTICE_KEY = "roalt.multiInstanceNotice";

function noticeSeen() {
  try {
    return localStorage.getItem(NOTICE_KEY) === "1";
  } catch {
    return false;
  }
}

/** Turns multi-instance on or off. The first time it is turned on, Roblox's stance on several
 * clients is shown first (DESIGN §2); render `dialog` next to the switch. */
export function useMultiToggle(onStatus?: (s: MultiInstanceStatus) => void) {
  const qc = useQueryClient();
  const [asking, setAsking] = useState(false);
  const set = (enabled: boolean) =>
    api.multiInstanceSet(enabled).then((s) => {
      qc.setQueryData(qk.multi, s);
      qc.invalidateQueries({ queryKey: qk.settings });
      onStatus?.(s);
    }, toastError);
  const toggle = (enabled: boolean) => (enabled && !noticeSeen() ? setAsking(true) : void set(enabled));
  const dialog = (
    <ConfirmDialog
      open={asking}
      onClose={() => setAsking(false)}
      onConfirm={() => {
        try {
          localStorage.setItem(NOTICE_KEY, "1");
        } catch {
          // Shown again next time; harmless.
        }
        setAsking(false);
        void set(true);
      }}
      title="Turn on multi-instance?"
      text={
        <>
          Roblox says it doesn't officially support running several clients at once. RoALT only holds the name Roblox's
          client checks when it starts and never touches the client itself, which Roblox says its enforcement against
          modified clients doesn't cover. Roblox has broken multi-instance before and may again.
        </>
      }
      confirmLabel="Turn on"
      safe
    />
  );
  return { toggle, dialog };
}

export function MultiInstanceSwitch() {
  const qc = useQueryClient();
  const status = useMultiStatus().data;
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);
  const apply = (s: MultiInstanceStatus) => qc.setQueryData(qk.multi, s);
  const { toggle, dialog } = useMultiToggle();
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
          <span role="alert" className="alert">{status.message}</span>
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
      {dialog}
    </>
  );
}
