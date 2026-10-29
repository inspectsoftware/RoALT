import { Fragment, useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api, type AccountSummary, type Crash, type CrashKind, type Instance, type InstanceState, type Limits, type MultiInstanceStatus } from "../../api";
import { qk, useBulk, useCrashes, useInstances, useMultiStatus, useSettings } from "../../data";
import { fmtList, t, type Key } from "../../i18n";
import { Segmented, Switch } from "../../ui/controls";
import { ConfirmDialog } from "../../ui/Dialog";
import { Divider, usePanelWidth } from "../../ui/Resizer";
import { Icon } from "../../ui/Icon";
import { toast, toastError } from "../../ui/Toast";
import { accountName, Avatar } from "../accounts/shared";
import { BatchSection } from "./Batch";
import { launchBlocked, launched, launchFailed } from "./LaunchView";

// Getters: labels are translated when read (at render), not once at load.
export const STATES: Record<InstanceState, [label: string, color: string]> = {
  get starting(): [string, string] { return [t("launch.state.starting"), "var(--dim)"]; },
  get joining(): [string, string] { return [t("launch.state.joining"), "var(--blue)"]; },
  get inGame(): [string, string] { return [t("launch.state.inGame"), "var(--accent-text)"]; },
  get disconnected(): [string, string] { return [t("launch.state.disconnected"), "var(--orange)"]; },
  get exited(): [string, string] { return [t("launch.state.ended"), "var(--dim)"]; },
};

export const isRunning = (i: Instance) => i.state !== "exited";

export function uptime(from: number, to: number) {
  const m = Math.max(0, Math.floor((to - from) / 60_000));
  return m < 60 ? t("launch.minutes", { m }) : t("launch.hoursMinutes", { h: Math.floor(m / 60), m: m % 60 });
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
/** The limits button's tooltip: what is limited, or what it opens. */
function limitsTitle(l: Limits) {
  if (!limited(l)) return t("launch.limits.buttonHint");
  const what = [
    l.cpuCap ? t("launch.limits.cpuPct", { pct: l.cpuCap }) : null,
    l.eco ? t("launch.limits.ecoItem") : null,
    l.trimEveryMin !== null ? t("launch.limits.trimItem") : null,
    l.affinity !== null ? t("launch.limits.coresItem") : null,
  ].filter((x): x is string => x !== null);
  return t("launch.limits.limited", { what: fmtList(what) });
}
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
    if (next === 0) return toastError({ kind: "invalid", message: t("launch.limits.oneCore") });
    void set({ affinity: next === allCores ? null : next });
  };
  const trimNow = () => {
    setTrimming(true);
    api
      .instanceTrim(inst.id)
      .then(() => toast(t("launch.limits.trimmed")), toastError)
      .finally(() => setTrimming(false));
  };
  return (
    <tr className="border-b border-line">
      <td colSpan={5} className="px-5 py-3 font-sans">
        <div className="flex flex-col gap-3">
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="text-[13px] text-text">{t("launch.limits.cpu")}</span>
            <Segmented
              mono
              label={t("launch.limits.cpu")}
              value={inst.limits.cpuCap ?? 0}
              options={CAPS.map((c) => ({ value: c, label: c ? t("launch.limits.pct", { pct: c }) : t("common.off") }))}
              onChange={(c) => set({ cpuCap: c || null })}
            />
          </div>
          <div className="flex items-center justify-between gap-2">
            <span className="text-[13px] text-text">{t("launch.limits.eco")}</span>
            <Switch label={t("launch.limits.eco")} checked={inst.limits.eco} onChange={(eco) => set({ eco })} />
          </div>
          <div className="flex flex-wrap items-center justify-between gap-2">
            <span className="text-[13px] text-text">{t("launch.limits.trim")}</span>
            <span className="flex flex-wrap items-center gap-2">
              <Segmented
                mono
                label={t("launch.limits.trimEvery")}
                value={inst.limits.trimEveryMin ?? -1}
                options={TRIMS.map((m) => ({ value: m, label: m < 0 ? t("common.default") : m ? t("launch.minutes", { m }) : t("common.off") }))}
                onChange={(m) => set({ trimEveryMin: m < 0 ? null : m })}
              />
              <button className="btn btn-sm" disabled={trimming} onClick={trimNow}>
                {trimming ? t("launch.limits.trimming") : t("launch.limits.trimNow")}
              </button>
            </span>
          </div>
          <fieldset className="flex flex-col gap-1.5">
            <legend className="flex w-full items-center justify-between gap-2 text-[13px] text-text">
              {t("launch.limits.cores")}
              {inst.limits.affinity !== null && (
                <button type="button" className="btn btn-sm" onClick={() => set({ affinity: null })}>{t("launch.limits.allCores")}</button>
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
              <span className="text-[13px] text-text">{t("launch.limits.antiAfk")}</span>
              <span className="mono text-xs text-dim">
                {!afk?.enabled ? t("launch.limits.afkOff") : inst.afkAt !== null ? t("launch.limits.lastKey", { ago: uptime(inst.afkAt, Date.now()) }) : t("launch.limits.noKey")}
              </span>
            </span>
            <Switch label={t("launch.limits.antiAfkClient")} checked={inst.antiAfk} onChange={(on) => api.instanceSetAntiAfk(inst.id, on).catch(toastError)} />
          </div>
          <p className="text-xs text-dim">
            {t("launch.limits.help")}
          </p>
        </div>
      </td>
    </tr>
  );
}

const KINDS: Record<CrashKind, Key> = {
  crash: "launch.crash.crash",
  disconnect: "launch.crash.disconnect",
  hang: "launch.crash.hang",
  mem: "launch.crash.mem",
};

/** What RoALT recorded about an ended client: how it ended, what its log said, its log. */
function CrashRow({ crash: c }: { crash: Crash }) {
  return (
    <tr className="border-b border-line">
      <td colSpan={5} className="px-5 py-3 font-sans">
        <div className="flex flex-col gap-2">
          <div className="flex flex-wrap items-baseline justify-between gap-2">
            <span className="text-[13px] text-text">{t(KINDS[c.kind])}</span>
            <span className="mono text-xs text-dim">{c.placeId !== null ? t("launch.crash.afterPlace", { time: uptime(0, c.uptimeMs), place: c.placeId }) : t("launch.crash.after", { time: uptime(0, c.uptimeMs) })}</span>
          </div>
          {c.reason && <p className="mono text-xs text-dim break-words selectable">{c.reason}</p>}
          {c.hasLog && (
            <button className="btn btn-sm self-start" onClick={() => api.crashOpenLog(c.id).catch(toastError)}>
              {t("launch.crash.openLog")}
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
      .launch(a.id, i.target, undefined, i.fpsCap)
      .then((info) => launched(a, info), launchFailed)
      .finally(() => setRelaunching(null));
  };
  // Starting next to running clients needs multi-instance, as a bulk launch does.
  const relaunchBlocked = launchBlocked(running.length > 0, multi, batch) ?? (relaunching ? t("launch.launching") : null);
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
    <Divider panel={width} side="right" label={t("launch.running.width")} />
    <aside aria-label={t("launch.running.title")} style={{ width: width.width }} className="@container flex-none min-w-0 flex flex-col bg-panel border-l border-line">
      <BatchSection accounts={accounts} onPick={onPick} />
      <div className="flex flex-wrap items-center gap-2.5 px-5 pt-[18px] pb-2.5">
        <span className="label whitespace-nowrap">{t("launch.running.title")}</span>
        <span className="mono text-xs text-dim">{running.length}</span>
        <span className="ml-auto flex gap-2">
          {running.length < list.length && (
            <button className="btn btn-sm" onClick={() => api.instancesClearEnded().catch(toastError)}>
              {t("launch.running.clearEnded")}
            </button>
          )}
          <button className="btn btn-sm" disabled={running.length === 0} onClick={() => setConfirmAll(true)}>
            {t("launch.running.closeAll")}
          </button>
        </span>
      </div>
      {rows.length === 0 ? (
        <p className="px-5 py-2 text-dim">{t("launch.running.empty")}</p>
      ) : (
        <div className="scroll flex-1 min-h-0">
          <table className="mono w-full border-collapse text-xs">
            <thead>
              <tr className="h-7 text-dim text-left">
                <th className="pl-5 font-normal border-b border-line">{t("launch.running.account")}</th>
                <th className="font-normal border-b border-line">{t("launch.running.state")}</th>
                <th className="font-normal border-b border-line hidden @sm:table-cell">{t("launch.running.job")}</th>
                <th className="font-normal border-b border-line">{t("launch.running.up")}</th>
                <th className="w-20 border-b border-line">
                  <span className="sr-only">{t("launch.running.actions")}</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {rows.map((i) => {
                const a = byId.get(i.accountId);
                const name = a ? accountName(a) : t("launch.accountN", { id: i.accountId });
                const [label, color] = STATES[i.state];
                const live = isRunning(i);
                const crash = live ? undefined : crashes.get(i.id);
                return (
                  <Fragment key={i.id}>
                  <tr className={`enter h-10 border-b border-line ${live ? "" : "text-dim"}`} title={i.afkAt !== null ? t("launch.running.pidAfk", { pid: i.pid, ago: uptime(i.afkAt, now) }) : t("launch.running.pid", { pid: i.pid })}>
                    <td className="pl-5 pr-2 max-w-0 w-[40%]">
                      <span className="flex items-center gap-2 min-w-0 font-sans">
                        {a && <Avatar account={a} size={20} />}
                        <span className="truncate">{name}</span>
                        {(i.fpsCap ?? a?.fpsCap) && (
                          <span className="mono flex-none text-dim" title={i.fpsCap ? t("launch.running.fpsLaunch") : t("launch.running.fpsAccount")}>
                            {t("launch.running.fps", { fps: (i.fpsCap ?? a?.fpsCap)! })}
                          </span>
                        )}

                        {i.rejoins > 0 && (
                          <span className="mono flex-none text-dim" title={t("launch.running.rejoined", { count: i.rejoins })}>
                            <span aria-hidden="true">↻{i.rejoins}</span>
                            <span className="sr-only">, {t("launch.running.rejoinedSr", { count: i.rejoins })}</span>
                          </span>
                        )}
                      </span>
                    </td>
                    <td className="pr-2 py-1.5">
                      <span className="flex items-baseline gap-1.5">
                        <span aria-hidden="true" className={`w-1.5 h-1.5 flex-none ${i.state === "starting" || i.state === "joining" ? "pulse" : ""}`} style={{ background: color }} />
                        <span>
                          {live ? (closing.has(closeKey(i)) ? t("launch.running.closing") : label) : i.note ?? label}
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
                          className={`wc w-9 h-8 ${i.throttled ? "text-orange" : limited(i.limits) ? "text-accent-text" : ""}`}
                          aria-label={t("launch.running.limitsFor", { name })}
                          aria-expanded={tuning === i.id}
                          title={[i.throttled ? t("launch.running.throttled") : null, limitsTitle(i.limits)].filter(Boolean).join(" ")}
                          onClick={() => setTuning((t) => (t === i.id ? null : i.id))}
                        >
                          <Icon name="sliders" size={12} />
                        </button>
                        <button
                          className="wc w-9 h-8"
                          aria-label={t("launch.running.closeName", { name })}
                          title={t("launch.running.closeThis")}
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
                            aria-label={t("launch.running.howEndedName", { name })}
                            aria-expanded={details === i.id}
                            title={t("launch.running.howEnded")}
                            onClick={() => setDetails((d) => (d === i.id ? null : i.id))}
                          >
                            <Icon name="chevron" size={12} className={details === i.id ? "rotate-180" : ""} />
                          </button>
                        )}
                        {a?.status === "valid" && !running.some((r) => r.accountId === a.id && r.state !== "disconnected") && (
                        <button
                          className="wc w-9 h-8"
                          aria-label={t("launch.running.relaunch", { name })}
                          title={relaunchBlocked ?? t("launch.running.relaunchHint")}
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
        title={t("launch.running.closeAllTitle")}
        text={t("launch.running.closeAllText", { count: running.length })}
        confirmLabel={t("launch.running.closeAll")}
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
        toast(t("launch.kill.done"));
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
      title={t("launch.kill.hint")}
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
      <span className="relative min-w-0 truncate">{busy ? t("launch.kill.killing") : held ? t("launch.kill.holding") : t("launch.kill.hold")}</span>
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
      title={t("launch.multi.turnOnTitle")}
      text={t("launch.multi.turnOnText")}
      confirmLabel={t("launch.multi.turnOn")}
      safe
    />
  );
  return { toggle, dialog };
}

export function MultiInstanceSwitch() {
  const qc = useQueryClient();
  const status = useMultiStatus().data;
  const [confirm, setConfirm] = useState<"close" | "takeOver" | null>(null);
  const [busy, setBusy] = useState(false);
  const apply = (s: MultiInstanceStatus) => qc.setQueryData(qk.multi, s);
  const { toggle, dialog } = useMultiToggle();
  const run = () => {
    setBusy(true);
    (confirm === "takeOver" ? api.multiInstanceTakeOver() : api.robloxCloseAll(false))
      .then(apply, toastError)
      .finally(() => {
        setBusy(false);
        setConfirm(null);
      });
  };

  if (!status) return null;
  const blocked = status.enabled && !status.active;
  return (
    <>
      <div className="flex items-center justify-between gap-2">
        <span className="font-sans text-[13px] text-text">{t("launch.multi.label")}</span>
        <Switch label={t("launch.multi.label")} checked={status.enabled} onChange={toggle} />
      </div>
      {blocked && (
        <div className="enter flex flex-col items-start gap-2 font-sans text-xs">
          <span role="alert" className="alert">{status.message}</span>
          {status.blockedBy.length > 0 && (
            <div className="flex flex-wrap gap-2">
              <button className="btn btn-sm" onClick={() => setConfirm("close")}>
                {t("launch.multi.closeRobloxEllipsis")}
              </button>
              <button className="btn btn-sm" onClick={() => setConfirm("takeOver")}>
                {t("launch.multi.takeOverEllipsis")}
              </button>
            </div>
          )}
        </div>
      )}
      <ConfirmDialog
        open={confirm === "close"}
        onClose={() => !busy && setConfirm(null)}
        onConfirm={run}
        busy={busy}
        title={t("launch.multi.closeTitle")}
        text={t("launch.multi.closeText", { count: status.blockedBy.length })}
        confirmLabel={t("launch.multi.closeRoblox")}
      />
      <ConfirmDialog
        open={confirm === "takeOver"}
        onClose={() => !busy && setConfirm(null)}
        onConfirm={run}
        busy={busy}
        title={t("launch.multi.takeOverTitle")}
        text={t("launch.multi.takeOverText", { count: status.blockedBy.length })}
        confirmLabel={t("launch.multi.takeOver")}
      />
      {dialog}
    </>
  );
}
