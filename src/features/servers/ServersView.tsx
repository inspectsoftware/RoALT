import { useEffect, useRef, useState, type Dispatch, type FormEvent, type ReactNode, type SetStateAction } from "react";
import { useInfiniteQuery, useQuery, useQueryClient, type InfiniteData } from "@tanstack/react-query";
import { api, asError, type AccountSummary, type GameInfo, type LaunchTarget, type ServerOrder } from "../../api";
import { qk, useBulk, useMultiStatus, useSettings } from "../../data";
import { Segmented, Switch } from "../../ui/controls";
import { Icon } from "../../ui/Icon";
import { friendly } from "../../ui/Toast";
import { accountName, avatarColor, plural } from "../accounts/shared";
import { launchBlocked, launched, launchFailed } from "../launch/LaunchView";
import { useNow } from "../launch/Running";
import { PrivateTable, PublicTable, short, type Pick } from "./ServerTables";

const PAGE = 25;
const SIGNED_IN: ServerOrder[] = ["recommended", "bestLatency"];
const ORDERS: { value: ServerOrder; label: string }[] = [
  { value: "occupancyDesc", label: "Most full" },
  { value: "occupancyAsc", label: "Most empty" },
  { value: "recommended", label: "Recommended" },
  { value: "bestLatency", label: "Best ping" },
];

/** The server browser. Roblox rate-limits the list, so pages load only when asked for. */
export function ServersView({ accounts, picked, setPicked, place, setPlace, onBatch, step, setStep }: {
  accounts: AccountSummary[];
  picked: number[];
  setPicked: Dispatch<SetStateAction<number[]>>;
  place: number | null;
  setPlace: (placeId: number) => void;
  /** A bulk launch started; its progress shows in the Launch view. */
  onBatch: () => void;
  /** Shared with the Launch view: one launch at a time, also across views. */
  step: string | null;
  setStep: (step: string | null) => void;
}) {
  const qc = useQueryClient();
  const multi = useMultiStatus().data;
  const batch = useBulk().data;
  const delayMs = useSettings().data?.launchDelayMs ?? 6000;
  const [text, setText] = useState(place === null ? "" : String(place));
  const [textError, setTextError] = useState<string | null>(null);
  const [tab, setTab] = useState<"public" | "private">("public");
  const [order, setOrder] = useState<ServerOrder>("occupancyDesc");
  const [hideFull, setHideFull] = useState(true);
  const [pick, setPick] = useState<Pick>(null);
  const [loading, setLoading] = useState(false);

  const valid = accounts.filter((a) => a.status === "valid");
  // In list order, which is also the bulk launch order; the first one asks Roblox.
  const chosen = valid.filter((a) => picked.includes(a.id));
  const asker = chosen[0]?.id;
  // Recommended and Best ping need an account: without one, nothing is asked for.
  const needsAccount = asker === undefined && SIGNED_IN.includes(order);

  // A game without its icon (failed or still being made) is asked for again next time.
  const game = useQuery({
    queryKey: qk.game(place),
    queryFn: () => api.gameInfo(place!),
    enabled: place !== null,
    staleTime: (q) => (q.state.data?.iconUrl ? Infinity : 0),
  });
  const g = game.data;
  // Not gated on g.privateServers: that guest hint is false for games whose servers exist anyway.
  const view = tab;
  const pubKey = [...qk.servers, place, order, hideFull];
  const pub = useInfiniteQuery({
    queryKey: pubKey,
    // The asking account isn't in the key: picking other accounts doesn't reload the list.
    // Every fetch path (Load next, Try again, refetch) goes through here: no guest request for a
    // signed-in sort.
    queryFn: ({ pageParam }) =>
      needsAccount
        ? Promise.reject({ kind: "invalid", message: "Pick an account: Roblox sorts this way only for a signed-in account." })
        : api.serversList(place!, order, hideFull, PAGE, pageParam ?? undefined, asker),
    initialPageParam: null as string | null,
    getNextPageParam: (p) => p.nextCursor,
    enabled: !!g && view === "public" && !needsAccount,
  });
  const privKey = [...qk.privateServers, place, asker];
  const priv = useInfiniteQuery({
    queryKey: privKey,
    queryFn: ({ pageParam }) => api.privateServersList(place!, asker!, pageParam ?? undefined),
    initialPageParam: null as string | null,
    getNextPageParam: (p) => p.nextCursor,
    enabled: !!g && view === "private" && asker !== undefined,
  });
  const list = view === "public" ? pub : priv;

  // Pages repeat servers as players come and go; the newest copy wins.
  const servers = [...new Map(pub.data?.pages.flatMap((p) => p.servers.map((s) => [s.id, s] as const)) ?? []).values()];
  const privates = priv.data?.pages.flatMap((p) => p.servers) ?? [];
  const count = view === "public" ? servers.length : privates.length;

  const pubPick = pick?.kind === "public" ? pick.id : null;
  const privPick = pick?.kind === "private" ? privates.find((s) => s.vipServerId === pick.id) : undefined;
  const room = pubPick ? servers.find((s) => s.id === pubPick) : privPick;
  const free = room ? Math.max(0, room.maxPlayers - room.playing) : null;
  const target: LaunchTarget | null =
    place === null ? null
    : pubPick ? { kind: "server", placeId: place, jobId: pubPick }
    : privPick?.accessCode ? { kind: "privateAccess", placeId: place, accessCode: privPick.accessCode }
    : null;
  const summary = pubPick
    ? `${short(pubPick)}${free === null ? "" : ` · ${free} free`}`
    : privPick ? `${privPick.name || "Private server"} · ${free} free` : "No server picked";
  const n = chosen.length;
  const askerName = chosen[0] ? accountName(chosen[0]) : "";
  const restricted = !!privPick && !!priv.data?.pages.some((p) => p.joinRestricted);
  const noJoin = `Roblox doesn't let ${askerName} join servers of this game.`;
  const blocked = launchBlocked(n > 1, multi, batch) ?? (restricted && n === 1 ? noJoin : null);
  const warning =
    [
      blocked ?? (restricted ? noJoin : null),
      privPick && n > 1 ? `Access was checked for ${askerName} only; accounts without access can't join.` : null,
      free !== null && n > free ? `Only ${plural(free, "free slot")} for ${plural(n, "account")}.` : null,
    ]
      .filter(Boolean)
      .join(" ") || null;
  const canJoin = !!target && n > 0 && !step && !blocked;

  // A lookup that finishes after a newer Load (or after leaving the view) is dropped.
  const loads = useRef(0);
  useEffect(() => () => void loads.current++, []);

  async function load(e: FormEvent) {
    e.preventDefault();
    const mine = ++loads.current;
    const p = await api.targetParse(text).catch(() => null);
    if (mine !== loads.current) return;
    const t = p?.kind === "target" ? p.target : null;
    if (!t || t.kind === "followUser") {
      setTextError(p && "reason" in p ? p.reason : "Paste a game link or place ID.");
      return;
    }
    // Universe and share links need a lookup (share links with the first picked account).
    setLoading(true);
    try {
      const id = "placeId" in t ? t.placeId : await api.targetPlace(t, asker);
      if (mine !== loads.current) return;
      // Loading the game already shown asks for its icon again if it was missing.
      if (id === place && g && !g.iconUrl) void game.refetch();
      setPlace(id);
      setTextError(null);
      setTab("public");
      setPick(t.kind === "server" ? { kind: "public", id: t.jobId } : null);
    } catch (e) {
      if (mine === loads.current) setTextError(friendly(asError(e)));
    } finally {
      if (mine === loads.current) setLoading(false);
    }
  }

  // Page 1 again, without asking for every page loaded so far.
  const refresh = () => {
    qc.setQueryData<InfiniteData<unknown>>(
      view === "public" ? pubKey : privKey,
      (d) => d && { pages: d.pages.slice(0, 1), pageParams: d.pageParams.slice(0, 1) },
      { updatedAt: list.dataUpdatedAt },
    );
    void list.refetch();
  };

  async function join() {
    if (!canJoin || !target) return;
    setStep(n > 1 ? "Starting the batch…" : "Getting a ticket…");
    try {
      if (n > 1) {
        await api.bulkLaunch({ accountIds: chosen.map((a) => a.id), target, delayMs, sameServer: "off", favouriteId: null });
        onBatch();
      } else launched(chosen[0], await api.launch(chosen[0].id, target));
    } catch (e) {
      launchFailed(e);
    } finally {
      setStep(null);
    }
  }

  const toggle = (id: number) => setPicked((p) => (p.includes(id) ? p.filter((x) => x !== id) : [...p, id]));
  const gameError = game.error && asError(game.error);

  return (
    <main className="flex-1 min-w-0 flex flex-col">
      <div className="flex-none flex flex-wrap items-center gap-x-5 gap-y-3 px-6 py-5 border-b border-line bg-panel">
        {g ? <GameHead game={g} /> : <h1 className="text-xl font-semibold tracking-[-0.01em]">Servers</h1>}
        <form className="flex flex-col gap-1.5 ml-auto flex-[0_1_340px] min-w-[220px]" onSubmit={(e) => void load(e)}>
          <div className="flex">
            <label htmlFor="servers-game" className="sr-only">Game link or place ID</label>
            <input
              id="servers-game"
              autoFocus={place === null}
              className="field mono flex-1 min-w-0"
              placeholder="Game link or place ID"
              spellCheck={false}
              autoComplete="off"
              value={text}
              aria-invalid={!!textError}
              aria-describedby={textError ? "servers-game-error" : undefined}
              onChange={(e) => {
                setText(e.target.value);
                setTextError(null);
              }}
            />
            <button className="btn border-l-0" disabled={loading}>{loading ? "Loading…" : "Load"}</button>
          </div>
          {textError && <p id="servers-game-error" role="alert" className="alert">{textError}</p>}
        </form>
      </div>

      {place === null ? (
        <Note>Paste a game link or place ID to see its servers.</Note>
      ) : game.isPending ? (
        <Note>
          <span className="flex items-center gap-2"><span aria-hidden="true" className="w-2 h-2 bg-dim pulse" />Loading the game…</span>
        </Note>
      ) : gameError ? (
        <Note>
          <span className="flex flex-col items-start gap-3">
            <span role="alert" className="alert">{gameError.kind === "notFound" ? "No game has that place ID." : friendly(gameError)}</span>
            {gameError.kind !== "notFound" && <button className="btn" onClick={() => void game.refetch()}>Try again</button>}
          </span>
        </Note>
      ) : (
        <>
          <div className="flex-none flex flex-wrap items-center gap-3 px-6 py-3.5">
            <Segmented
              label="Server type"
              value={view}
              options={[{ value: "public", label: "Public" }, { value: "private", label: "Private" }]}
              onChange={(v) => {
                setTab(v);
                setPick(null);
              }}
            />
            {view === "public" && (
              <>
                <Segmented
                  label="Sort servers"
                  value={order}
                  options={ORDERS.map((o) =>
                    asker === undefined && SIGNED_IN.includes(o.value)
                      ? { ...o, disabled: true, title: "Pick an account: Roblox sorts this way only for a signed-in account." }
                      : o,
                  )}
                  onChange={setOrder}
                />
                <span className="flex items-center gap-2.5">
                  <Switch label="Hide full servers" checked={hideFull} onChange={setHideFull} />
                  <span aria-hidden="true">Hide full</span>
                </span>
              </>
            )}
            <span className="flex items-center gap-3 ml-auto">
              {list.isFetching ? (
                <span className="mono text-xs text-dim">loading…</span>
              ) : (
                list.dataUpdatedAt > 0 && <Updated at={list.dataUpdatedAt} />
              )}
              <button className="btn" disabled={list.isFetching || asker === undefined && (view === "private" || needsAccount)} onClick={refresh}>
                <Icon name="refresh" size={14} className={list.isFetching ? "spin" : ""} />
                Refresh
              </button>
            </span>
            <span role="status" className="sr-only">
              {list.isFetching ? "Loading servers" : list.isSuccess ? `${plural(count, "server")} listed` : ""}
            </span>
          </div>

          <div className="scroll flex-1 min-h-0 border-t border-line">
            {view === "public" && needsAccount && !pub.data ? (
              <p className="px-6 py-4 text-dim">Pick an account below: Roblox sorts this way only for a signed-in account.</p>
            ) : view === "public" ? (
              <PublicTable servers={servers} pick={pubPick} onPick={(id) => setPick({ kind: "public", id })} />
            ) : asker === undefined ? (
              <p className="px-6 py-4 text-dim">Pick an account below: Roblox lists the private servers that account can see.</p>
            ) : (
              <>
                <p className="px-6 pt-3 text-xs text-dim">As {askerName}</p>
                {priv.data?.pages.some((p) => p.joinRestricted) && (
                  <p className="alert mx-6 my-3">Roblox doesn't let {askerName} join servers of this game.</p>
                )}
                <PrivateTable servers={privates} pick={privPick?.vipServerId ?? null} onPick={(id) => setPick({ kind: "private", id })} />
              </>
            )}
            <div className="flex flex-col items-center gap-3 p-3.5 text-dim">
              {list.isError ? (
                <>
                  <p role="alert" className="alert">{friendly(asError(list.error))}</p>
                  <button className="btn" disabled={list.isFetching || (view === "public" && needsAccount)} onClick={() => void (list.isFetchNextPageError ? list.fetchNextPage() : refresh())}>
                    Try again
                  </button>
                </>
              ) : list.isFetching && !list.isFetchingNextPage && count === 0 ? (
                "Loading servers…"
              ) : list.isSuccess && count === 0 ? (
                view === "private" ? `No private servers ${askerName} can see.` : hideFull ? "No servers with room right now." : "No servers right now."
              ) : (
                list.hasNextPage && (
                  <button className="btn" disabled={list.isFetching || (view === "public" && needsAccount)} onClick={() => void list.fetchNextPage()}>
                    {list.isFetchingNextPage ? "Loading…" : `Load next ${PAGE}`}
                  </button>
                )
              )}
            </div>
          </div>

          <div className="flex-none px-6 py-3 border-t border-line-strong bg-raised">
            <div className="flex flex-wrap items-center gap-2.5">
              <span className="mono font-medium">{summary}</span>
              <span aria-hidden="true" className="text-dim">send</span>
              {valid.length ? (
                <div role="group" aria-label="Accounts to send" className="flex flex-wrap gap-1.5">
                  {valid.map((a) => (
                    <button key={a.id} type="button" className="chip chip-sm" aria-pressed={picked.includes(a.id)} onClick={() => toggle(a.id)}>
                      <span aria-hidden="true" className="w-2 h-2" style={{ background: avatarColor(a) }} />
                      {accountName(a)}
                    </button>
                  ))}
                </div>
              ) : (
                <span className="text-dim">no account with a valid cookie</span>
              )}
              <button
                className="btn btn-primary ml-auto"
                disabled={!canJoin}
                title={target ? (blocked ?? undefined) : "Pick a server first"}
                onClick={() => void join()}
              >
                <Icon name="play" size={12} fill />
                {step ? "Joining…" : `Join with ${plural(n, "account")}`}
              </button>
            </div>
            <div aria-live="polite">{warning && <p className="alert mt-2">{warning}</p>}</div>
          </div>
        </>
      )}
    </main>
  );
}

function Note({ children }: { children: ReactNode }) {
  return <div className="flex-1 grid place-items-center p-8 text-dim">{children}</div>;
}

function Updated({ at }: { at: number }) {
  const s = Math.max(0, Math.round((useNow(1000) - at) / 1000));
  return <span className="mono text-xs text-dim">updated {s < 60 ? `${s} s` : `${Math.floor(s / 60)} min`} ago</span>;
}

function GameHead({ game: g }: { game: GameInfo }) {
  return (
    <div className="flex items-center gap-5 min-w-0">
      {g.iconUrl ? (
        <img src={g.iconUrl} alt="" className="w-16 h-16 flex-none object-cover border border-line-strong" />
      ) : (
        <div aria-hidden="true" className="gridbg w-16 h-16 flex-none grid place-content-center border border-line-strong" style={{ backgroundSize: "8px 8px" }}>
          <span className="mono text-[10px] text-dim">ICON</span>
        </div>
      )}
      <div className="flex flex-col gap-1 min-w-0">
        <div className="flex items-baseline gap-2 min-w-0">
          <h1 className="text-xl font-semibold tracking-[-0.01em] truncate min-w-0">{g.name}</h1>
          <span className="text-dim truncate flex-none max-w-[40%]">by {g.creator}</span>
        </div>
        <span className="mono text-xs text-dim selectable">place {g.placeId} · universe {g.universeId}</span>
        <span className="flex items-center gap-1.5">
          <span aria-hidden="true" className="w-1.5 h-1.5 bg-accent-text" />
          <span className="mono">{g.playing.toLocaleString()}</span>
          <span className="text-dim">playing</span>
        </span>
      </div>
    </div>
  );
}
