import { useMemo, useRef, useState } from "react";
import type { PcRuntime, RemoteManager } from "../transport";
import type { LaunchTarget, ParsedInput } from "../types";
import { parseTargetParse } from "../validate";

function describeTarget(t: LaunchTarget): string {
  switch (t.kind) {
    case "place":
      return `Place ${t.placeId}${t.launchData ? " with launch data" : ""}`;
    case "server":
      return `Server ${t.jobId} in place ${t.placeId}`;
    case "privateLink":
      return `Private server link in place ${t.placeId}`;
    case "privateAccess":
      return `Private server in place ${t.placeId}`;
    case "shareLink":
      return `Share link (${t.linkType}) ${t.code}`;
    case "shareLinkV2":
      return `Share link ${t.code}`;
    case "universe":
      return `Experience ${t.universeId}`;
    case "followUser":
      return `Join user ${t.userId}`;
  }
}

export function LaunchView({
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
  const [linkInput, setLinkInput] = useState("");
  const [parsed, setParsed] = useState<ParsedInput | null>(null);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [groupFilter, setGroupFilter] = useState<number | "all">("all");
  const [search, setSearch] = useState("");
  const [fpsCap, setFpsCap] = useState("");
  const [launching, setLaunching] = useState(false);
  const parseSeq = useRef(0);

  const visibleAccounts = useMemo(() => {
    const q = search.trim().toLowerCase();
    return snap.accounts.filter((a) => {
      if (groupFilter !== "all" && !a.groupIds.includes(groupFilter)) return false;
      if (!q) return true;
      return (
        a.username.toLowerCase().includes(q) ||
        a.displayName.toLowerCase().includes(q) ||
        a.alias.toLowerCase().includes(q)
      );
    });
  }, [snap.accounts, groupFilter, search]);

  const toggle = (id: number) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const selectAllVisible = () => {
    setSelected(new Set(visibleAccounts.filter((a) => !a.main && a.status !== "invalid").map((a) => a.id)));
  };

  const parseInput = async (val: string) => {
    const seq = ++parseSeq.current;
    setLinkInput(val);
    setParsed(null);
    if (!val.trim()) return;
    let result: ParsedInput | null = null;
    try {
      result = parseTargetParse(await manager.rpc(rt.pc.pcId, { op: "targetParse", input: val }));
    } catch {
      result = null;
    }
    if (seq === parseSeq.current) setParsed(result);
  };

  const launchFavourite = async (favouriteId: number) => {
    if (readonly) return;
    setLaunching(true);
    try {
      await manager.rpc(rt.pc.pcId, { op: "launchFavourite", favouriteId, accountIds: [...selected], fpsCap: fpsCap ? Number(fpsCap) : null });
      onToast("Launching favourite");
    } catch (e: unknown) {
      onToast(e instanceof Error ? e.message : String(e), true);
    } finally {
      setLaunching(false);
    }
  };

  const startLaunch = async (target: LaunchTarget, ids: number[]) => {
    if (readonly || ids.length === 0) return;
    setLaunching(true);
    const cap = fpsCap ? Number(fpsCap) : null;
    try {
      if (ids.length === 1) {
        await manager.rpc(rt.pc.pcId, { op: "launch", accountId: ids[0], target, fpsCap: cap });
        onToast("Launched account");
      } else {
        await manager.rpc(rt.pc.pcId, {
          op: "bulkLaunch",
          request: { accountIds: ids, target, delayMs: 6000, sameServer: "off", fpsCap: cap },
        });
        onToast(`Started batch for ${ids.length} accounts`);
      }
    } catch (e: unknown) {
      onToast(e instanceof Error ? e.message : String(e), true);
    } finally {
      setLaunching(false);
    }
  };

  const bulk = snap.bulkProgress;

  return (
    <div className="space-y-4">
      {bulk && !bulk.done && (
        <div className="glass-card flex items-center justify-between gap-3 rounded-2xl border-lime-400/30 p-4">
          <div>
            <div className="text-sm font-semibold text-white">Bulk launch in progress</div>
            <div className="text-xs text-zinc-400">
              {bulk.items.filter((e) => e.stage.kind === "inGame" || e.stage.kind === "done").length} / {bulk.items.length} done ·{" "}
              {bulk.items.filter((e) => e.stage.kind === "failed").length} failed
            </div>
          </div>
          {!readonly && (
            <button
              type="button"
              onClick={() => void manager.rpc(rt.pc.pcId, { op: "bulkCancel", batchId: bulk.batchId }).catch(() => {})}
              className="rounded-xl border border-red-500/40 bg-red-500/15 px-3 py-1.5 text-xs text-red-300"
            >
              Cancel batch
            </button>
          )}
        </div>
      )}

      {/* Game / link input */}
      <div className="glass-card space-y-3 rounded-2xl p-4">
        <div className="text-sm font-semibold text-white">Game, server, or private server link</div>
        <div className="flex flex-col gap-2 sm:flex-row">
          <input
            type="text"
            value={linkInput}
            onChange={(e) => void parseInput(e.target.value)}
            placeholder="Paste Roblox game URL, Place ID, share link, or private server link…"
            className="flex-1 rounded-xl border border-white/10 bg-black/40 px-3.5 py-2.5 text-sm text-white placeholder:text-zinc-500 focus:border-lime-400 focus:outline-none"
          />
          <input
            type="number"
            min={5}
            max={360}
            value={fpsCap}
            onChange={(e) => setFpsCap(e.target.value)}
            placeholder="FPS cap"
            className="w-full rounded-xl border border-white/10 bg-black/40 px-3 py-2.5 text-sm text-white placeholder:text-zinc-500 sm:w-28"
          />
        </div>
        {parsed && parsed.kind === "target" && (
          <div className="break-all text-xs text-lime-300">Ready to launch: {describeTarget(parsed.target)}</div>
        )}
        {parsed && (parsed.kind === "username" || parsed.kind === "jobIdOnly") && (
          <div className="text-xs text-amber-300">Paste a game, server or share link. Usernames and bare server IDs only work in the RoALT app.</div>
        )}
        {parsed && (parsed.kind === "invalid" || parsed.kind === "unsupported") && (
          <div className="text-xs text-red-300">{parsed.reason}</div>
        )}

        {/* Saved favourites */}
        {snap.favourites.length > 0 && (
          <div className="pt-1">
            <div className="mb-1.5 text-xs text-zinc-400">Saved favourites</div>
            <div className="flex flex-wrap gap-1.5">
              {snap.favourites.map((fav) => (
                <button
                  key={fav.id}
                  type="button"
                  disabled={readonly || launching || snap.vaultLocked}
                  onClick={() => void launchFavourite(fav.id)}
                  className="rounded-lg border border-white/10 bg-white/5 px-3 py-1.5 text-xs text-zinc-200 hover:border-lime-400/40 hover:bg-lime-400/10"
                >
                  ★ {fav.name}
                </button>
              ))}
            </div>
          </div>
        )}
      </div>

      {/* Account selection */}
      <div className="glass-card space-y-3 rounded-2xl p-4">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <div className="flex flex-wrap items-center gap-1.5">
            <button
              type="button"
              onClick={() => setGroupFilter("all")}
              className={`rounded-full px-3 py-1 text-xs ${groupFilter === "all" ? "bg-lime-400 font-semibold text-zinc-950" : "bg-white/5 text-zinc-300"}`}
            >
              All ({snap.accounts.length})
            </button>
            {snap.groups.map((g) => (
              <button
                key={g.id}
                type="button"
                onClick={() => setGroupFilter(g.id)}
                className={`rounded-full px-3 py-1 text-xs ${groupFilter === g.id ? "bg-lime-400 font-semibold text-zinc-950" : "bg-white/5 text-zinc-300"}`}
              >
                {g.name}
              </button>
            ))}
          </div>
          <div className="flex items-center gap-2">
            <button type="button" onClick={selectAllVisible} className="text-xs text-lime-300 hover:underline">
              Select alts
            </button>
            <button type="button" onClick={() => setSelected(new Set())} className="text-xs text-zinc-400 hover:underline">
              Clear
            </button>
          </div>
        </div>

        <input
          type="text"
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder="Filter accounts…"
          className="w-full rounded-xl border border-white/10 bg-black/40 px-3.5 py-2 text-sm text-white placeholder:text-zinc-500"
        />

        <div className="grid max-h-[380px] grid-cols-1 gap-1.5 overflow-y-auto sm:grid-cols-2">
          {visibleAccounts.map((a) => {
            const checked = selected.has(a.id);
            const name = a.alias || a.displayName || a.username;
            return (
              <button
                key={a.id}
                type="button"
                onClick={() => toggle(a.id)}
                className={`flex items-center justify-between rounded-xl border px-3 py-2 text-left transition ${
                  checked ? "border-lime-400/50 bg-lime-400/10" : "border-white/5 bg-black/25 hover:bg-white/5"
                }`}
              >
                <div className="min-w-0">
                  <div className="flex items-center gap-1.5">
                    {a.color && <span className="h-2 w-2 rounded-full" style={{ background: a.color }} />}
                    <span className="truncate text-sm font-medium text-white">{name}</span>
                    {a.main && <span className="rounded bg-white/10 px-1.5 py-0.5 text-[10px] text-zinc-300">MAIN</span>}
                  </div>
                  <div className="truncate text-xs text-zinc-500">@{a.username}</div>
                </div>
                <span
                  className={`h-4 w-4 rounded border ${
                    checked ? "border-lime-400 bg-lime-400" : "border-zinc-600"
                  }`}
                />
              </button>
            );
          })}
        </div>

        {!readonly && parsed?.kind === "target" && (
          <div className="break-all text-xs text-zinc-400">Target: {describeTarget(parsed.target)}</div>
        )}
        {!readonly && (
          <button
            type="button"
            disabled={launching || snap.vaultLocked || selected.size === 0 || parsed?.kind !== "target"}
            onClick={() => parsed?.kind === "target" && void startLaunch(parsed.target, [...selected])}
            className="w-full rounded-xl bg-lime-400 py-3 text-sm font-semibold text-zinc-950 transition hover:bg-lime-300 disabled:opacity-40"
          >
            {snap.vaultLocked
              ? "Vault is locked on PC"
              : launching
                ? "Launching…"
                : `Launch ${selected.size} account${selected.size === 1 ? "" : "s"}`}
          </button>
        )}
      </div>
    </div>
  );
}
