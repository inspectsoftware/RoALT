import { useState } from "react";
import type { PcRuntime, RemoteManager } from "../transport";

export function MonitorView({
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
  const [shutdownMins, setShutdownMins] = useState("60");
  const [panicConfirm, setPanicConfirm] = useState(false);

  const run = async (cmd: Record<string, unknown>, okMsg: string) => {
    try {
      await manager.rpc(rt.pc.pcId, cmd);
      onToast(okMsg);
    } catch (e: unknown) {
      onToast(e instanceof Error ? e.message : String(e), true);
    }
  };

  const memPct = snap.systemUsage ? Math.min(100, (snap.systemUsage.memUsedMb / Math.max(1, snap.systemUsage.memTotalMb)) * 100) : 0;

  return (
    <div className="space-y-4">
      {/* System stats */}
      <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
        <div className="glass-card rounded-2xl p-4">
          <div className="text-xs text-zinc-400">PC CPU Usage</div>
          <div className="mt-1 font-mono text-2xl font-bold text-white">
            {snap.systemUsage?.cpuPct != null ? `${Math.round(snap.systemUsage.cpuPct)}%` : "—"}
          </div>
          <div className="mt-2 h-2 overflow-hidden rounded-full bg-white/10">
            <div className="h-full bg-lime-400 transition-all" style={{ width: `${Math.min(100, snap.systemUsage?.cpuPct ?? 0)}%` }} />
          </div>
        </div>
        <div className="glass-card rounded-2xl p-4">
          <div className="text-xs text-zinc-400">PC Memory</div>
          <div className="mt-1 font-mono text-2xl font-bold text-white">
            {snap.systemUsage
              ? `${(snap.systemUsage.memUsedMb / 1024).toFixed(1)} / ${(snap.systemUsage.memTotalMb / 1024).toFixed(0)} GB`
              : "—"}
          </div>
          <div className="mt-2 h-2 overflow-hidden rounded-full bg-white/10">
            <div className="h-full bg-sky-400 transition-all" style={{ width: `${memPct}%` }} />
          </div>
        </div>
      </div>

      {/* Scheduled Shutdown & Panic Kill */}
      {!readonly && (
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
          <div className="glass-card space-y-3 rounded-2xl p-4">
            <div className="text-sm font-semibold text-white">Scheduled PC Shutdown</div>
            {snap.shutdownAt ? (
              <div className="space-y-2">
                <div className="text-xs text-amber-300">
                  Scheduled for {new Date(snap.shutdownAt).toLocaleTimeString()}
                </div>
                <button
                  type="button"
                  onClick={() => void run({ op: "shutdownCancel" }, "Shutdown cancelled")}
                  className="rounded-xl border border-white/15 px-3.5 py-2 text-xs text-white hover:bg-white/10"
                >
                  Cancel shutdown
                </button>
              </div>
            ) : (
              <div className="flex items-center gap-2">
                <input
                  type="number"
                  min={1}
                  max={10080}
                  value={shutdownMins}
                  onChange={(e) => setShutdownMins(e.target.value)}
                  className="w-24 rounded-xl border border-white/10 bg-black/40 px-3 py-2 text-sm text-white"
                />
                <span className="text-xs text-zinc-400">min</span>
                <button
                  type="button"
                  onClick={() =>
                    void run(
                      { op: "shutdownSchedule", inSecs: Math.max(60, Math.round(Number(shutdownMins) * 60) || 0), closeClients: true },
                      `PC shutdown scheduled in ${shutdownMins} min`,
                    )
                  }
                  className="ml-auto rounded-xl border border-white/15 bg-white/5 px-3.5 py-2 text-xs text-white hover:bg-white/10"
                >
                  Schedule
                </button>
              </div>
            )}
          </div>

          <div className="glass-card flex flex-col justify-between gap-3 rounded-2xl border-red-500/25 p-4">
            <div>
              <div className="text-sm font-semibold text-red-300">Panic Kill All</div>
              <p className="mt-1 text-xs text-zinc-400">
                Cancels batches, disables auto-rejoin, and immediately terminates every Roblox client on <bdi>{rt.pc.pcName}</bdi>.
              </p>
            </div>
            {!panicConfirm ? (
              <button
                type="button"
                onClick={() => setPanicConfirm(true)}
                className="w-full rounded-xl border border-red-500/40 bg-red-500/15 py-2 text-xs font-semibold text-red-300 hover:bg-red-500/25"
              >
                Panic Kill All Roblox Clients
              </button>
            ) : (
              <div className="flex gap-2">
                <button
                  type="button"
                  onClick={() => setPanicConfirm(false)}
                  className="flex-1 rounded-xl border border-white/15 py-2 text-xs text-zinc-300"
                >
                  Cancel
                </button>
                <button
                  type="button"
                  onClick={() => {
                    setPanicConfirm(false);
                    void run({ op: "panicKill", confirmed: true }, "Panic kill executed");
                  }}
                  className="flex-1 rounded-xl bg-red-600 py-2 text-xs font-semibold text-white hover:bg-red-500"
                >
                  Confirm Kill All
                </button>
              </div>
            )}
          </div>
        </div>
      )}

      {/* Recent crashes */}
      <div className="glass-card rounded-2xl p-4">
        <div className="mb-3 text-sm font-semibold text-white">Recent crashes & disconnects ({snap.crashes.length})</div>
        {snap.crashes.length === 0 ? (
          <div className="text-xs text-zinc-500">No recorded crashes.</div>
        ) : (
          <div className="space-y-1.5">
            {snap.crashes.slice(0, 20).map((c) => {
              const acc = snap.accounts.find((a) => a.id === c.accountId);
              return (
                <div key={c.id} className="flex items-center justify-between gap-3 rounded-xl bg-black/30 px-3 py-2 text-xs">
                  <div className="min-w-0">
                    <span className="font-medium text-zinc-200">{acc?.alias || acc?.username || `#${c.accountId}`}</span>
                    <span className="ml-2 text-zinc-400">{c.reason || c.kind}</span>
                  </div>
                  <span className="shrink-0 font-mono text-[11px] text-zinc-500">{new Date(c.at).toLocaleTimeString()}</span>
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}
