import { useEffect, useMemo, useRef, useState } from "react";
import { ClientsView } from "./components/ClientsView";
import { LaunchView } from "./components/LaunchView";
import { MonitorView } from "./components/MonitorView";
import { PairView } from "./components/PairView";
import { PcBoundary } from "./components/PcBoundary";
import { confirmPairedPc, fallBackStalePairings, loadPairedPcs, removePairedPc, upsertPairedPc, type PairedPc } from "./storage";
import { RemoteManager, type PcRuntime, type RelayIssue } from "./transport";
import { nameKey } from "./validate";

type Tab = "clients" | "launch" | "monitor";

const RELAY_ISSUE_TEXT: Record<string, string> = {
  frame_too_large: "A message was too large for the relay and was dropped.",
  rate_limited: "The relay is slowing this browser down. Wait a moment before trying again.",
  bad_version: "The relay dropped a message it could not read. Reload this page.",
};

type QrHash = { pairId: string; secretB64: string };

function readAndScrubQrHash(): QrHash | null {
  const m = (window.location.hash || "").match(/^#pair=([a-zA-Z0-9_-]+)\.([a-zA-Z0-9_-]+)/);
  if (!m) return null;
  window.history.replaceState(null, "", window.location.pathname + window.location.search);
  return { pairId: m[1], secretB64: m[2] };
}

export function App() {
  const [loaded, setLoaded] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loadTry, setLoadTry] = useState(0);
  const [pcs, setPcs] = useState<PairedPc[]>([]);
  const [selectedPcId, setSelectedPcId] = useState<string | null>(null);
  const [pairingMode, setPairingMode] = useState(false);
  const [qrHash, setQrHash] = useState<QrHash | null>(() => readAndScrubQrHash());
  const [tab, setTab] = useState<Tab>("clients");
  const [runtimes, setRuntimes] = useState<Record<string, PcRuntime>>({});
  const [toastMsg, setToastMsg] = useState<{ text: string; err: boolean } | null>(null);
  const [upgradeRequired, setUpgradeRequired] = useState(false);
  const managerRef = useRef<RemoteManager | null>(null);
  const confirming = useRef(new Set<string>());

  const showToast = (text: string, err = false) => {
    setToastMsg({ text, err });
    window.setTimeout(() => setToastMsg((cur) => (cur?.text === text ? null : cur)), 3500);
  };

  useEffect(() => {
    let alive = true;
    setLoadError(null);
    loadPairedPcs().then((savedPcs) => {
      if (!alive) return;
      setLoaded(true);
      setPcs(savedPcs);
      if (savedPcs.length > 0) setSelectedPcId(savedPcs[0].pcId);
      const mgr = new RemoteManager();
      managerRef.current = mgr;
      let shownIssue: RelayIssue | null = null;
      mgr.subscribe(() => {
        setRuntimes(mgr.getRuntimes());
        setUpgradeRequired(mgr.isUpgradeRequired());
        const issue = mgr.getRelayIssue();
        if (issue && issue !== shownIssue) {
          shownIssue = issue;
          showToast(RELAY_ISSUE_TEXT[issue.code] ?? issue.code, true);
        }
      });
      mgr.setPcs(savedPcs);
    }, (err: unknown) => {
      if (alive) setLoadError(err instanceof Error ? err.message : String(err));
    });
    return () => {
      alive = false;
      managerRef.current?.destroy();
    };
  }, [loadTry]);

  useEffect(() => {
    for (const pc of pcs) {
      const rt = runtimes[pc.pcId];
      if (!pc.previous || rt?.status !== "online") continue;
      const key = `${pc.pcId}/${rt.pc.deviceId}`;
      if (confirming.current.has(key)) continue;
      confirming.current.add(key);
      void confirmPairedPc(pc.pcId, rt.pc.deviceId)
        .then((next) => {
          setPcs(next);
          managerRef.current?.setPcs(next);
        })
        .catch((err: unknown) => console.warn("remote: could not save the confirmed pairing", err))
        .finally(() => confirming.current.delete(key));
    }
  }, [pcs, runtimes]);

  useEffect(() => {
    if (!loaded) return;
    const settle = () =>
      void fallBackStalePairings(Date.now()).then(
        (next) => {
          if (!next) return;
          setPcs(next);
          managerRef.current?.setPcs(next);
        },
        (err: unknown) => console.warn("remote: could not switch an unconfirmed pairing", err),
      );
    const timer = window.setInterval(settle, 60_000);
    return () => window.clearInterval(timer);
  }, [loaded]);

  const handlePaired = async (pc: PairedPc) => {
    const next = await upsertPairedPc(pc);
    setPcs(next);
    setSelectedPcId(pc.pcId);
    setQrHash(null);
    setPairingMode(false);
    managerRef.current?.setPcs(next);
    showToast(`Paired with ${pc.pcName}`);
  };

  const handleForgetPc = async (pcId: string) => {
    const next = await removePairedPc(pcId);
    setPcs(next);
    managerRef.current?.setPcs(next);
    if (selectedPcId === pcId) setSelectedPcId(next[0]?.pcId ?? null);
    showToast("Removed paired PC from this browser");
  };

  const activeRt = selectedPcId ? runtimes[selectedPcId] ?? null : null;

  const nameClash = useMemo(() => {
    const owners = new Map<string, Set<string>>();
    for (const pc of pcs) {
      for (const name of [pc.pcName, runtimes[pc.pcId]?.snapshot?.pcName]) {
        if (!name) continue;
        const k = nameKey(name);
        owners.set(k, (owners.get(k) ?? new Set()).add(pc.pcId));
      }
    }
    const out = new Set<string>();
    for (const ids of owners.values()) if (ids.size > 1) for (const id of ids) out.add(id);
    return out;
  }, [pcs, runtimes]);

  const totalRunning = useMemo(() => {
    let sum = 0;
    for (const rt of Object.values(runtimes)) {
      if (rt.status === "online" && rt.snapshot) {
        sum += rt.snapshot.instances.filter((i) => i.state !== "exited" || i.relaunching).length;
      }
    }
    return sum;
  }, [runtimes]);

  if (loadError) {
    return (
      <div className="flex min-h-screen flex-col items-center justify-center gap-3 px-6 text-center text-sm">
        <div className="font-semibold text-red-300">Could not open this browser's encrypted storage</div>
        <p className="max-w-md text-zinc-400">
          {loadError} Private windows and blocked site data can stop it too; this dashboard needs site storage to keep your paired PCs.
        </p>
        <button type="button" onClick={() => setLoadTry((n) => n + 1)} className="rounded-lg bg-lime-400 px-4 py-1.5 text-xs font-semibold text-zinc-950">
          Try again
        </button>
      </div>
    );
  }

  if (!loaded) {
    return (
      <div className="flex min-h-screen items-center justify-center text-sm text-zinc-400">
        Initializing encrypted storage…
      </div>
    );
  }

  if (qrHash || pairingMode || pcs.length === 0) {
    return (
      <div className="min-h-screen bg-[#0b0d10]">
        <PairView
          qrHash={qrHash}
          pairedPcs={pcs}
          canCancel={pcs.length > 0 || !!qrHash}
          onCancel={() => {
            setQrHash(null);
            setPairingMode(false);
          }}
          onPaired={handlePaired}
        />
      </div>
    );
  }

  return (
    <div className="min-h-screen bg-[#0b0d10] pb-20 sm:pb-8">
      {/* Top Header + Multi-PC Switcher */}
      <header className="sticky top-0 z-30 border-b border-white/10 bg-[#0b0d10]/90 backdrop-blur-md">
        <div className="mx-auto flex max-w-6xl flex-wrap items-center justify-between gap-3 px-4 py-3">
          <div className="flex items-center gap-2.5">
            <img src="/icon.svg" alt="RoALT" className="h-8 w-8 rounded-lg" />
            <div>
              <div className="flex items-center gap-2 text-sm font-bold tracking-tight text-white">
                <span>RoALT Remote</span>
                <span className="rounded-full bg-white/10 px-2 py-0.5 font-mono text-[10px] font-normal text-zinc-300">
                  E2EE · {totalRunning} active
                </span>
              </div>
            </div>
          </div>

          {/* Multi-PC Pills */}
          <div className="flex flex-wrap items-center gap-1.5">
            {pcs.map((pc) => {
              const rt = runtimes[pc.pcId];
              const st = rt?.status ?? "connecting";
              const count = rt?.snapshot?.instances.filter((i) => i.state !== "exited" || i.relaunching).length ?? 0;
              const isSel = pc.pcId === selectedPcId;
              const liveName = rt?.snapshot?.pcName;
              const renamed = !!liveName && liveName !== pc.pcName;
              const dotCls =
                st === "online"
                  ? "bg-emerald-400 shadow-[0_0_8px_rgba(52,211,153,0.7)]"
                  : st === "connecting"
                    ? "bg-amber-400"
                    : st === "tooMany"
                      ? "bg-red-500"
                      : st === "tooManyTabs" || st === "deviceBusy"
                        ? "bg-sky-400"
                        : "bg-zinc-600";
              return (
                <button
                  key={pc.pcId}
                  type="button"
                  onClick={() => setSelectedPcId(pc.pcId)}
                  title={renamed ? `Paired as “${pc.pcName}”. This PC now calls itself “${liveName}”.` : undefined}
                  className={`flex items-center gap-2 rounded-full border px-3 py-1.5 text-xs font-medium transition ${
                    isSel
                      ? "border-lime-400/50 bg-lime-400/15 text-white"
                      : "border-white/10 bg-white/5 text-zinc-300 hover:bg-white/10"
                  }`}
                >
                  <span className={`h-2 w-2 rounded-full ${dotCls}`} />
                  <bdi>{pc.pcName}</bdi>
                  {renamed && <span className="rounded bg-amber-400/15 px-1 text-[10px] text-amber-300">renamed</span>}
                  {nameClash.has(pc.pcId) && (
                    <span className="rounded bg-amber-400/15 px-1 text-[10px] text-amber-300">
                      same name · <span className="font-mono">#{pc.deviceId.slice(0, 4)}</span>
                    </span>
                  )}
                  {st === "online" && <span className="font-mono text-[10px] text-lime-300">{count}</span>}
                </button>
              );
            })}
            <button
              type="button"
              onClick={() => setPairingMode(true)}
              className="rounded-full border border-dashed border-white/20 px-3 py-1.5 text-xs text-zinc-300 hover:border-lime-400/50 hover:text-white"
            >
              + Pair PC
            </button>
          </div>
        </div>

        {/* Desktop navigation tabs */}
        {activeRt?.status === "online" && activeRt.snapshot && (
          <div className="mx-auto hidden max-w-6xl items-center justify-between border-t border-white/5 px-4 sm:flex">
            <div className="flex gap-1 py-1.5">
              {(["clients", "launch", "monitor"] as Tab[]).map((t) => (
                <button
                  key={t}
                  type="button"
                  onClick={() => setTab(t)}
                  className={`rounded-lg px-3.5 py-1.5 text-xs font-medium capitalize transition ${
                    tab === t ? "bg-white/10 text-white" : "text-zinc-400 hover:text-zinc-200"
                  }`}
                >
                  {t}
                </button>
              ))}
            </div>
            <div className="flex items-center gap-3 text-xs text-zinc-500">
              {activeRt.snapshot.pcName !== activeRt.pc.pcName && (
                <span className="max-w-[16rem] truncate text-amber-300">
                  Now calls itself “<bdi>{activeRt.snapshot.pcName}</bdi>”
                </span>
              )}
              <span>
                RoALT v{activeRt.snapshot.appVersion} ·{" "}
                {activeRt.snapshot.role === "readonly" ? "View only" : "Full control"}
              </span>
              <button
                type="button"
                onClick={() => void handleForgetPc(activeRt.pc.pcId)}
                className="text-zinc-500 hover:text-red-400"
              >
                Forget PC
              </button>
            </div>
          </div>
        )}
      </header>

      {/* Main content */}
      <main className="mx-auto max-w-6xl px-4 py-4">
        {upgradeRequired && (
          <div className="mb-4 flex flex-wrap items-center justify-between gap-2 rounded-xl border border-amber-400/30 bg-amber-400/10 px-4 py-2.5 text-xs text-amber-200">
            <span>
              <strong>This dashboard is out of date.</strong> Reload the page to update it.
            </span>
            <button type="button" onClick={() => window.location.reload()} className="rounded-lg bg-amber-400 px-3 py-1 font-semibold text-zinc-950">
              Reload
            </button>
          </div>
        )}
        {!activeRt ? null : activeRt.status === "tooMany" ? (
          <div className="glass-card mx-auto max-w-md rounded-2xl p-8 text-center">
            <div className="text-base font-semibold text-red-300">Too many PCs on this relay</div>
            <p className="mt-1.5 text-sm text-zinc-400">
              One browser can follow up to 16 PCs on the same relay. Forget a PC you no longer use, then reload this page.
            </p>
            <button
              type="button"
              onClick={() => void handleForgetPc(activeRt.pc.pcId)}
              className="mt-5 rounded-xl border border-white/10 px-4 py-2 text-xs text-zinc-400 hover:text-red-300"
            >
              Forget this PC
            </button>
          </div>
        ) : activeRt.status === "tooManyTabs" ? (
          <div className="glass-card mx-auto max-w-md rounded-2xl p-8 text-center">
            <div className="text-base font-semibold text-white">
              <bdi>{activeRt.pc.pcName}</bdi> is open in 3 other tabs
            </div>
            <p className="mt-1.5 text-sm text-zinc-400">
              This browser can follow one PC from 3 tabs at a time. This tab connects as soon as you close one of the others.
            </p>
          </div>
        ) : activeRt.status === "paused" ? (
          <div className="glass-card mx-auto max-w-md rounded-2xl p-8 text-center">
            <div className="text-base font-semibold text-white">
              <bdi>{activeRt.pc.pcName}</bdi> paused Remote
            </div>
            <p className="mt-1.5 text-sm text-zinc-400">This page connects again once Remote is resumed in RoALT on that PC.</p>
            <button
              type="button"
              onClick={() => void handleForgetPc(activeRt.pc.pcId)}
              className="mt-5 rounded-xl border border-white/10 px-4 py-2 text-xs text-zinc-400 hover:text-red-300"
            >
              Forget this PC
            </button>
          </div>
        ) : activeRt.status === "deviceBusy" ? (
          <div className="glass-card mx-auto max-w-md rounded-2xl p-8 text-center">
            <div className="text-base font-semibold text-white">Too many sessions for this device</div>
            <p className="mt-1.5 text-sm text-zinc-400">
              This browser already has 3 sessions open on <bdi>{activeRt.pc.pcName}</bdi>. This one connects as soon as one of them ends.
            </p>
            <button
              type="button"
              onClick={() => void handleForgetPc(activeRt.pc.pcId)}
              className="mt-5 rounded-xl border border-white/10 px-4 py-2 text-xs text-zinc-400 hover:text-red-300"
            >
              Forget this PC
            </button>
          </div>
        ) : activeRt.status !== "online" || !activeRt.snapshot ? (
          <div className="glass-card mx-auto max-w-md rounded-2xl p-8 text-center">
            <div className="text-base font-semibold text-white">
              {activeRt.status === "connecting" ? `Connecting to ${activeRt.pc.pcName}…` : `${activeRt.pc.pcName} is offline`}
            </div>
            {activeRt.problem && <p className="mt-1.5 text-sm text-amber-300">{activeRt.problem}</p>}
            <p className="mt-1.5 text-sm text-zinc-400">
              Make sure RoALT is running on your Windows PC and Remote Dashboard is enabled in Settings → Remote. If you revoked this
              browser in RoALT, forget this PC here and pair it again.
            </p>
            <button
              type="button"
              onClick={() => void handleForgetPc(activeRt.pc.pcId)}
              className="mt-5 rounded-xl border border-white/10 px-4 py-2 text-xs text-zinc-400 hover:text-red-300"
            >
              Forget this PC
            </button>
          </div>
        ) : (
          <PcBoundary key={activeRt.pc.pcId} pcName={activeRt.pc.pcName} onForget={() => void handleForgetPc(activeRt.pc.pcId)}>
            <div className="space-y-4">
              {activeRt.problem && (
                <div className="rounded-xl border border-amber-400/30 bg-amber-400/10 px-4 py-2.5 text-xs text-amber-200">{activeRt.problem}</div>
              )}
              {activeRt.lastNotice?.error && (
                <div className="line-clamp-3 break-words rounded-xl border border-amber-400/30 bg-amber-400/10 px-4 py-2.5 text-xs text-amber-200">
                  {activeRt.lastNotice.message}
                </div>
              )}
              {activeRt.snapshot.windowsLocked && (
                <div className="rounded-xl border border-amber-400/30 bg-amber-400/10 px-4 py-2.5 text-xs text-amber-200">
                  <strong>Windows is locked</strong> — management commands still work, but Windows pauses live previews while locked. Turn off your monitor instead of locking for live view.
                </div>
              )}
              {activeRt.snapshot.vaultLocked && (
                <div className="rounded-xl border border-sky-400/30 bg-sky-400/10 px-4 py-2.5 text-xs text-sky-200">
                  <strong>
                    Vault is locked on <bdi>{activeRt.pc.pcName}</bdi>
                  </strong>{" "}
                  — monitoring and closing clients still work, but launching or rejoining requires unlocking RoALT on the PC.
                </div>
              )}
              {activeRt.snapshot.role === "readonly" && (
                <div className="rounded-xl border border-white/10 bg-white/5 px-4 py-2 text-xs text-zinc-300">
                  This device is paired as <strong>View only</strong>. Change permissions in RoALT Settings → Remote to enable remote actions.
                </div>
              )}

              {tab === "clients" && <ClientsView key={activeRt.pc.pcId} rt={activeRt} manager={managerRef.current!} onToast={showToast} />}
              {tab === "launch" && <LaunchView key={activeRt.pc.pcId} rt={activeRt} manager={managerRef.current!} onToast={showToast} />}
              {tab === "monitor" && <MonitorView key={activeRt.pc.pcId} rt={activeRt} manager={managerRef.current!} onToast={showToast} />}
            </div>
          </PcBoundary>
        )}
        {activeRt?.status === "online" && activeRt.snapshot && (
          <div className="mt-6 flex justify-center sm:hidden">
            <button type="button" onClick={() => void handleForgetPc(activeRt.pc.pcId)} className="text-xs text-zinc-500 hover:text-red-400">
              Forget <bdi>{activeRt.pc.pcName}</bdi>
            </button>
          </div>
        )}
      </main>

      {/* Mobile bottom tab bar */}
      {activeRt?.status === "online" && activeRt.snapshot && (
        <nav className="fixed inset-x-0 bottom-0 z-30 grid grid-cols-3 border-t border-white/10 bg-[#0b0d10]/95 backdrop-blur-md sm:hidden">
          {(["clients", "launch", "monitor"] as Tab[]).map((t) => (
            <button
              key={t}
              type="button"
              onClick={() => setTab(t)}
              className={`py-3 text-xs font-semibold capitalize ${tab === t ? "text-lime-400" : "text-zinc-400"}`}
            >
              {t}
            </button>
          ))}
        </nav>
      )}

      {/* Toast */}
      {toastMsg && (
        <div
          className={`fixed bottom-20 left-1/2 z-50 -translate-x-1/2 rounded-xl border px-4 py-2.5 text-xs font-medium shadow-xl sm:bottom-6 ${
            toastMsg.err
              ? "border-red-500/40 bg-red-950/95 text-red-200"
              : "border-lime-400/30 bg-zinc-900/95 text-zinc-100"
          }`}
        >
          {toastMsg.text}
        </div>
      )}
    </div>
  );
}
