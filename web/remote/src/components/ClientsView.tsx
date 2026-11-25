import { useEffect, useMemo, useRef, useState } from "react";
import type { PcRuntime, RemoteManager } from "../transport";
import { ControlPad } from "./ControlPad";
import { LivePreview } from "./LivePreview";
import type { ClientMetrics, Limits, RemoteAccount, RemoteInstance, RemotePreviewTarget } from "../types";

function fmtUptime(startedAt: number, endedAt: number | null): string {
  const ms = Math.max(0, (endedAt ?? Date.now()) - startedAt);
  const mins = Math.floor(ms / 60_000);
  if (mins < 60) return `${mins}m`;
  const h = Math.floor(mins / 60);
  return `${h}h ${mins % 60}m`;
}

/** Why a card has no picture, from the PC's preview status (`instances::window::Status`). */
function previewText(status: number | undefined, off: boolean, big = false): string {
  if (off) return "Live previews are off for View-only devices on this PC";
  if (status === 1) return "Window minimized";
  if (status === 2) return "No game window yet";
  if (status === 4) return "This window can't be captured on the PC";
  return big ? "Waiting for the live view…" : "Waiting for frame…";
}

const IDLE_MS = 2 * 60_000;
const TILE_FPS = 2;
const TILE_W = 240;

function useIdle(ms: number): boolean {
  const [idle, setIdle] = useState(false);
  useEffect(() => {
    let timer = window.setTimeout(() => setIdle(true), ms);
    const wake = () => {
      window.clearTimeout(timer);
      setIdle(false);
      timer = window.setTimeout(() => setIdle(true), ms);
    };
    const events = ["pointerdown", "keydown", "wheel", "scroll", "touchstart"] as const;
    for (const e of events) window.addEventListener(e, wake, { passive: true, capture: true });
    return () => {
      window.clearTimeout(timer);
      for (const e of events) window.removeEventListener(e, wake, { capture: true });
    };
  }, [ms]);
  return idle;
}

function stateBadge(inst: RemoteInstance): { label: string; cls: string } {
  if (inst.relaunching) return { label: "Rejoining", cls: "bg-amber-400/15 text-amber-300 border-amber-400/30" };
  switch (inst.state) {
    case "inGame":
      return { label: "In game", cls: "bg-emerald-400/15 text-emerald-300 border-emerald-400/30" };
    case "joining":
    case "starting":
      return { label: inst.state === "starting" ? "Starting" : "Joining", cls: "bg-sky-400/15 text-sky-300 border-sky-400/30" };
    case "disconnected":
      return { label: "Disconnected", cls: "bg-red-500/20 text-red-300 border-red-400/30" };
    case "exited":
      return { label: "Exited", cls: "bg-zinc-700/40 text-zinc-400 border-white/10" };
  }
}

export function ClientsView({
  rt,
  manager,
  onToast,
}: {
  rt: PcRuntime;
  manager: RemoteManager;
  onToast: (msg: string, err?: boolean) => void;
}) {
  const snap = rt.snapshot!;
  const readonly = snap.role === "readonly";
  const [focusedId, setFocusedId] = useState<string | null>(null);
  const [inspectorFps, setInspectorFps] = useState(0);
  useEffect(() => setInspectorFps(0), [focusedId]);
  const [limitsInst, setLimitsInst] = useState<RemoteInstance | null>(null);
  const [visibleIds, setVisibleIds] = useState<Set<string>>(new Set());
  const [busyId, setBusyId] = useState<string | null>(null);
  const cardRefs = useRef(new Map<string, HTMLElement>());

  const activeInstances = useMemo(
    () => snap.instances.filter((i) => i.state !== "exited" || i.relaunching),
    [snap.instances],
  );
  const endedInstances = useMemo(
    () => snap.instances.filter((i) => i.state === "exited" && !i.relaunching).slice(0, 12),
    [snap.instances],
  );
  const accountMap = useMemo(() => {
    const m = new Map<number, RemoteAccount>();
    for (const a of snap.accounts) m.set(a.id, a);
    return m;
  }, [snap.accounts]);
  const metricsMap = useMemo(() => {
    const m = new Map<string, ClientMetrics>();
    for (const cm of snap.metrics) m.set(cm.instanceId, cm);
    return m;
  }, [snap.metrics]);

  // Observe visible cards so we only stream live JPEG thumbnails for cards currently on screen.
  useEffect(() => {
    const io = new IntersectionObserver(
      (entries) => {
        setVisibleIds((prev) => {
          const next = new Set(prev);
          for (const e of entries) {
            const id = (e.target as HTMLElement).dataset.instId;
            if (!id) continue;
            if (e.isIntersecting) next.add(id);
            else next.delete(id);
          }
          return next;
        });
      },
      { rootMargin: "120px" },
    );
    for (const el of cardRefs.current.values()) io.observe(el);
    return () => io.disconnect();
  }, [activeInstances.length]);

  // A hidden tab or locked phone isn't watching: the PC stops capturing (on Windows 10 that also
  // takes the yellow border off the game).
  const idle = useIdle(IDLE_MS);
  const [pageVisible, setPageVisible] = useState(() => document.visibilityState === "visible");
  useEffect(() => {
    const onChange = () => setPageVisible(document.visibilityState === "visible");
    document.addEventListener("visibilitychange", onChange);
    return () => document.removeEventListener("visibilitychange", onChange);
  }, []);

  // Stop this PC's previews when the page closes or switches PC, not between updates (that would blank every thumbnail).
  useEffect(() => () => manager.setPreviewTargets(rt.pc.pcId, []), [rt.pc.pcId]); // eslint-disable-line react-hooks/exhaustive-deps

  // Push preview subscription to PC whenever viewport visibility or focused modal changes.
  useEffect(() => {
    if (rt.status !== "online" || rt.previewsOff || snap.windowsLocked || !pageVisible || idle) {
      manager.setPreviewTargets(rt.pc.pcId, []);
      return;
    }
    const targets: RemotePreviewTarget[] = [];
    if (focusedId && activeInstances.some((i) => i.id === focusedId)) {
      targets.push({ instanceId: focusedId, maxW: 640, fps: 30 });
    } else {
      for (const inst of activeInstances) {
        if (visibleIds.size === 0 || visibleIds.has(inst.id)) {
          targets.push({ instanceId: inst.id, maxW: TILE_W, fps: TILE_FPS });
        }
        if (targets.length >= 12) break;
      }
    }
    manager.setPreviewTargets(rt.pc.pcId, targets);
  }, [rt.pc.pcId, rt.status, rt.previewsOff, snap.windowsLocked, pageVisible, idle, focusedId, activeInstances.map((i) => i.id).join(","), [...visibleIds].sort().join(",")]); // eslint-disable-line react-hooks/exhaustive-deps

  const run = async (id: string, cmd: Record<string, unknown>, okMsg?: string) => {
    setBusyId(id);
    try {
      await manager.rpc(rt.pc.pcId, cmd);
      if (okMsg) onToast(okMsg);
    } catch (e: unknown) {
      onToast(e instanceof Error ? e.message : String(e), true);
    } finally {
      setBusyId(null);
    }
  };

  const focusedInst = focusedId ? activeInstances.find((i) => i.id === focusedId) ?? null : null;

  return (
    <div className="space-y-4">
      {/* Toolbar */}
      <div className="flex flex-wrap items-center justify-between gap-2">
        <div className="text-sm text-zinc-400">
          <span className="font-semibold text-white">{activeInstances.length}</span> active client
          {activeInstances.length === 1 ? "" : "s"}
          {snap.systemUsage && (
            <span className="ml-3 font-mono text-xs text-zinc-400">
              PC CPU {snap.systemUsage.cpuPct != null ? `${Math.round(snap.systemUsage.cpuPct)}%` : "—"} · RAM{" "}
              {(snap.systemUsage.memUsedMb / 1024).toFixed(1)}/{(snap.systemUsage.memTotalMb / 1024).toFixed(0)} GB
            </span>
          )}
        </div>
        {!readonly && activeInstances.length > 0 && (
          <div className="flex flex-wrap gap-1.5">
            <button
              type="button"
              onClick={() => void run("all", { op: "instancesShowAll", what: "minimize" }, "Minimized all clients")}
              className="rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-xs text-zinc-200 hover:bg-white/10"
            >
              Minimize all
            </button>
            <button
              type="button"
              onClick={() => void run("all", { op: "instancesShowAll", what: "restore" }, "Restored all clients")}
              className="rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-xs text-zinc-200 hover:bg-white/10"
            >
              Restore all
            </button>
            <button
              type="button"
              onClick={() => void run("all", { op: "instancesTile", micro: false }, "Tiled windows")}
              className="rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-xs text-zinc-200 hover:bg-white/10"
            >
              Tile grid
            </button>
            <button
              type="button"
              onClick={() => void run("all", { op: "instancesCloseAll" }, "Closing all clients")}
              className="rounded-lg border border-red-500/30 bg-red-500/10 px-2.5 py-1.5 text-xs text-red-300 hover:bg-red-500/20"
            >
              Close all
            </button>
          </div>
        )}
      </div>

      {idle && activeInstances.length > 0 && !rt.previewsOff && (
        <div className="rounded-xl border border-sky-400/30 bg-sky-400/10 px-4 py-2.5 text-xs text-sky-200">
          Live previews paused to save data. Tap anywhere to resume.
        </div>
      )}

      {/* Active client cards */}
      {activeInstances.length === 0 ? (
        <div className="glass-card rounded-2xl p-10 text-center">
          <div className="text-base font-medium text-white">No active Roblox clients</div>
          <p className="mt-1 text-sm text-zinc-400">Switch to the Launch tab to start one or more alt accounts on <bdi>{rt.pc.pcName}</bdi>.</p>
        </div>
      ) : (
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {activeInstances.map((inst) => {
            const acc = accountMap.get(inst.accountId);
            const met = metricsMap.get(inst.id);
            const prev = rt.previews[inst.id];
            const badge = stateBadge(inst);
            const name = acc?.alias || acc?.displayName || acc?.username || `Account #${inst.accountId}`;

            return (
              <div
                key={inst.id}
                data-inst-id={inst.id}
                ref={(el) => {
                  if (el) cardRefs.current.set(inst.id, el);
                  else cardRefs.current.delete(inst.id);
                }}
                className="glass-card flex flex-col overflow-hidden rounded-2xl"
              >
                {/* Live preview */}
                <button
                  type="button"
                  onClick={() => setFocusedId(inst.id)}
                  className="group relative aspect-video w-full overflow-hidden bg-zinc-950 text-left"
                >
                  {prev?.picture && !snap.windowsLocked ? (
                    <LivePreview
                      manager={manager}
                      pcId={rt.pc.pcId}
                      instanceId={inst.id}
                      aria-label={name}
                      className="h-full w-full object-cover transition group-hover:scale-[1.02]"
                    />
                  ) : (
                    <div className="flex h-full w-full flex-col items-center justify-center gap-1 px-4 text-center text-xs text-zinc-500">
                      <span>{snap.windowsLocked ? "PC locked — preview paused" : previewText(prev?.status, rt.previewsOff)}</span>
                    </div>
                  )}
                  <div className="absolute left-2.5 top-2.5 flex items-center gap-1.5">
                    <span className={`rounded-full border px-2 py-0.5 text-[11px] font-medium backdrop-blur ${badge.cls}`}>
                      {badge.label}
                    </span>
                    {inst.antiAfk && (
                      <span className="rounded-full border border-lime-400/30 bg-lime-400/15 px-2 py-0.5 text-[10px] font-medium text-lime-300 backdrop-blur">
                        AFK
                      </span>
                    )}
                  </div>
                  <div className="absolute bottom-2 right-2 rounded bg-black/70 px-1.5 py-0.5 font-mono text-[10px] text-zinc-300">
                    {fmtUptime(inst.startedAt, inst.endedAt)}
                  </div>
                </button>

                {/* Metadata & quick controls */}
                <div className="flex flex-1 flex-col gap-2.5 p-3.5">
                  <div className="flex items-start justify-between gap-2">
                    <div className="min-w-0">
                      <div className="flex items-center gap-1.5">
                        {acc?.color && <span className="h-2 w-2 rounded-full" style={{ background: acc.color }} />}
                        <span className="truncate text-sm font-semibold text-white">{name}</span>
                      </div>
                      <div className="truncate text-xs text-zinc-400">
                        @{acc?.username ?? "—"}
                        {inst.placeId ? ` · Place ${inst.placeId}` : ""}
                      </div>
                    </div>
                    {met && (
                      <div className="text-right font-mono text-[11px] text-zinc-400">
                        {met.cpuPct != null && <div>{met.cpuPct.toFixed(0)}% CPU</div>}
                        <div>{Math.round(met.memoryMb)} MB</div>
                      </div>
                    )}
                  </div>

                  {inst.note && <div className="rounded-lg bg-amber-500/10 px-2.5 py-1.5 text-xs text-amber-200">{inst.note}</div>}

                  {!readonly && (
                    <div className="mt-auto flex flex-wrap gap-1.5 pt-1">
                      <button
                        type="button"
                        disabled={busyId === inst.id}
                        onClick={() => void run(inst.id, { op: "instanceRejoin", id: inst.id }, `Rejoining ${name}`)}
                        className="rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-xs text-zinc-200 hover:bg-white/10"
                      >
                        Rejoin
                      </button>
                      <button
                        type="button"
                        disabled={busyId === inst.id}
                        onClick={() =>
                          void run(inst.id, { op: "instanceSetAntiAfk", id: inst.id, enabled: !inst.antiAfk }, `Anti-AFK ${inst.antiAfk ? "off" : "on"}`)
                        }
                        className={`rounded-lg border px-2.5 py-1.5 text-xs ${
                          inst.antiAfk
                            ? "border-lime-400/30 bg-lime-400/10 text-lime-300"
                            : "border-white/10 bg-white/5 text-zinc-300"
                        }`}
                      >
                        AFK {inst.antiAfk ? "On" : "Off"}
                      </button>
                      <button
                        type="button"
                        onClick={() => setLimitsInst(inst)}
                        className="rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-xs text-zinc-200 hover:bg-white/10"
                      >
                        Limits
                      </button>
                      <button
                        type="button"
                        disabled={busyId === inst.id}
                        onClick={() => void run(inst.id, { op: "instanceShow", id: inst.id, what: "focus" }, "Focused window")}
                        className="rounded-lg border border-white/10 bg-white/5 px-2.5 py-1.5 text-xs text-zinc-200 hover:bg-white/10"
                      >
                        Focus
                      </button>
                      <button
                        type="button"
                        disabled={busyId === inst.id}
                        onClick={() => void run(inst.id, { op: "instanceClose", id: inst.id, force: false }, `Closing ${name}`)}
                        className="ml-auto rounded-lg border border-red-500/30 bg-red-500/10 px-2.5 py-1.5 text-xs text-red-300 hover:bg-red-500/20"
                      >
                        Close
                      </button>
                    </div>
                  )}
                </div>
              </div>
            );
          })}
        </div>
      )}

      {/* Recently ended */}
      {endedInstances.length > 0 && (
        <div className="space-y-2 pt-2">
          <div className="text-xs font-medium uppercase tracking-wider text-zinc-500">Recently ended</div>
          <div className="grid grid-cols-1 gap-2 sm:grid-cols-2">
            {endedInstances.map((inst) => {
              const acc = accountMap.get(inst.accountId);
              const name = acc?.alias || acc?.displayName || acc?.username || `#${inst.accountId}`;
              return (
                <div key={inst.id} className="glass-card flex items-center justify-between gap-3 rounded-xl px-3.5 py-2.5">
                  <div className="min-w-0">
                    <div className="truncate text-sm text-zinc-200">{name}</div>
                    <div className="truncate text-xs text-zinc-500">{inst.note || "Ended"}</div>
                  </div>
                  {!readonly && (
                    <button
                      type="button"
                      onClick={() =>
                        void run(inst.id, { op: "instanceRejoin", id: inst.id }, `Relaunching ${name}`)
                      }
                      className="rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-lime-300 hover:bg-white/10"
                    >
                      Relaunch
                    </button>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      )}

      {/* Enlarged live inspector modal */}
      {focusedInst && (
        <div className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/80 p-2 backdrop-blur-sm sm:items-center sm:p-4" onClick={() => setFocusedId(null)}>
          <div className="glass-card w-full max-w-3xl overflow-hidden rounded-2xl" onClick={(e) => e.stopPropagation()}>
            <div className="flex items-center justify-between border-b border-white/10 px-4 py-3">
              <div className="font-semibold text-white">
                {accountMap.get(focusedInst.accountId)?.alias || accountMap.get(focusedInst.accountId)?.username} · Live Inspector{inspectorFps ? ` (${inspectorFps} FPS)` : ""}
              </div>
              <button type="button" onClick={() => setFocusedId(null)} className="rounded-lg px-2 py-1 text-sm text-zinc-400 hover:bg-white/10">
                ✕
              </button>
            </div>
            {readonly ? (
              <div className="aspect-video w-full bg-black">
                {rt.previews[focusedInst.id]?.picture ? (
                  <LivePreview
                    manager={manager}
                    pcId={rt.pc.pcId}
                    instanceId={focusedInst.id}
                    onFps={setInspectorFps}
                    aria-label="Live view"
                    className="h-full w-full object-contain"
                  />
                ) : (
                  <div className="flex h-full items-center justify-center px-6 text-center text-sm text-zinc-500">{previewText(rt.previews[focusedInst.id]?.status, rt.previewsOff, true)}</div>
                )}
              </div>
            ) : (
              <ControlPad
                manager={manager}
                pcId={rt.pc.pcId}
                instanceId={focusedInst.id}
                live={!!rt.previews[focusedInst.id]?.picture}
                onFps={setInspectorFps}
                emptyText={previewText(rt.previews[focusedInst.id]?.status, rt.previewsOff, true)}
                onError={(msg) => onToast(msg, true)}
              />
            )}
            {!readonly && (
              <div className="flex flex-wrap items-center gap-2 border-t border-white/10 p-3.5">
                <button
                  type="button"
                  onClick={() => void run(focusedInst.id, { op: "instanceShow", id: focusedInst.id, what: "restore" })}
                  className="rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-zinc-200 hover:bg-white/10"
                >
                  Restore
                </button>
                <button
                  type="button"
                  onClick={() => void run(focusedInst.id, { op: "instanceShow", id: focusedInst.id, what: "minimize" })}
                  className="rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-zinc-200 hover:bg-white/10"
                >
                  Minimize
                </button>
                <button
                  type="button"
                  onClick={() => void run(focusedInst.id, { op: "instanceTrim", id: focusedInst.id }, "Trimmed memory")}
                  className="rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-zinc-200 hover:bg-white/10"
                >
                  Trim RAM
                </button>
                <button
                  type="button"
                  onClick={() => void run(focusedInst.id, { op: "instanceClose", id: focusedInst.id, force: true }, "Force killed client")}
                  className="ml-auto rounded-lg border border-red-500/40 bg-red-500/15 px-3 py-1.5 text-xs text-red-300 hover:bg-red-500/25"
                >
                  Force kill
                </button>
              </div>
            )}
          </div>
        </div>
      )}

      {/* Per-client limits dialog */}
      {limitsInst && (
        <LimitsModal
          inst={limitsInst}
          onClose={() => setLimitsInst(null)}
          onSave={async (limits) => {
            await run(limitsInst.id, { op: "instanceSetLimits", id: limitsInst.id, limits }, "Limits updated");
            setLimitsInst(null);
          }}
        />
      )}
    </div>
  );
}

function LimitsModal({
  inst,
  onClose,
  onSave,
}: {
  inst: RemoteInstance;
  onClose: () => void;
  onSave: (l: Limits) => Promise<void>;
}) {
  const [cpuCap, setCpuCap] = useState<string>(inst.limits.cpuCap ? String(inst.limits.cpuCap) : "");
  const [eco, setEco] = useState(inst.limits.eco);
  const [trimEveryMin, setTrimEveryMin] = useState<string>(inst.limits.trimEveryMin == null ? "" : String(inst.limits.trimEveryMin));

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/75 p-4 backdrop-blur-sm" onClick={onClose}>
      <div className="glass-card w-full max-w-sm rounded-2xl p-5" onClick={(e) => e.stopPropagation()}>
        <h3 className="text-base font-semibold text-white">Client performance limits</h3>
        <div className="mt-4 space-y-3">
          <div>
            <label className="mb-1 block text-xs text-zinc-400">CPU cap % (empty = unlimited)</label>
            <input
              type="number"
              min={1}
              max={100}
              value={cpuCap}
              onChange={(e) => setCpuCap(e.target.value)}
              placeholder="e.g. 25"
              className="w-full rounded-xl border border-white/10 bg-black/40 px-3 py-2 text-sm text-white"
            />
          </div>
          <div>
            <label className="mb-1 block text-xs text-zinc-400">Memory trim interval (minutes, 0 = off, empty = app default)</label>
            <input
              type="number"
              min={1}
              max={240}
              value={trimEveryMin}
              onChange={(e) => setTrimEveryMin(e.target.value)}
              placeholder="e.g. 15"
              className="w-full rounded-xl border border-white/10 bg-black/40 px-3 py-2 text-sm text-white"
            />
          </div>
          <label className="flex items-center justify-between rounded-xl border border-white/10 bg-black/30 px-3 py-2.5 text-sm text-zinc-200">
            <span>Windows Efficiency mode (EcoQoS)</span>
            <input type="checkbox" checked={eco} onChange={(e) => setEco(e.target.checked)} className="h-4 w-4 accent-lime-400" />
          </label>
        </div>
        <div className="mt-5 flex justify-end gap-2">
          <button type="button" onClick={onClose} className="rounded-xl border border-white/10 px-4 py-2 text-xs text-zinc-300">
            Cancel
          </button>
          <button
            type="button"
            onClick={() =>
              void onSave({
                cpuCap: cpuCap ? Math.min(100, Math.max(1, Number(cpuCap))) : null,
                eco,
                trimEveryMin: trimEveryMin.trim() === "" ? null : Math.min(240, Math.max(0, Math.round(Number(trimEveryMin)) || 0)),
                affinity: inst.limits.affinity,
              })
            }
            className="rounded-xl bg-lime-400 px-4 py-2 text-xs font-semibold text-zinc-950"
          >
            Apply
          </button>
        </div>
      </div>
    </div>
  );
}
