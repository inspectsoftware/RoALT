import { useCallback, useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { useQuery } from "@tanstack/react-query";
import { api, type AccountSummary, type ClientMetrics, type Instance, type Limits, type WindowShow } from "../../api";
import { qk, useInstances, useMetrics, useSettings } from "../../data";
import { Icon, type IconName } from "../../ui/Icon";
import { toast, toastError } from "../../ui/Toast";
import { accountName, Avatar, plural } from "../accounts/shared";
import { isRunning, KillAllButton, STATES, uptime, useNow } from "../launch/Running";
import { mb, pct } from "../monitor/MonitorView";

/** A picture is asked for this often per card, one at a time. */
const THUMB_MS = 2000;
const CAPS = [0, 50, 25, 10, 5];

/** Draws the client's window on `canvas` every 2 s while the RoALT window is visible and
 * focused; true / false once it is known whether there is a picture (false: minimized or no
 * window yet), null before. */
function useThumbnail(id: string, canvas: React.RefObject<HTMLCanvasElement | null>) {
  const [shown, setShown] = useState<boolean | null>(null);
  useEffect(() => {
    let stop = false;
    let timer: number | undefined;
    const tick = async () => {
      const el = canvas.current;
      if (el && document.visibilityState === "visible" && document.hasFocus()) {
        try {
          const buf = await api.instanceThumbnail(id, Math.min(480, Math.max(160, Math.round(el.clientWidth * devicePixelRatio))));
          if (stop) return;
          if (buf.byteLength < 8) setShown(false);
          else {
            const head = new DataView(buf);
            const [w, h] = [head.getUint32(0, true), head.getUint32(4, true)];
            el.width = w;
            el.height = h;
            el.getContext("2d")?.putImageData(new ImageData(new Uint8ClampedArray(buf, 8, w * h * 4), w, h), 0, 0);
            setShown(true);
          }
        } catch {
          // Ended or not started: the card goes with the next instances list.
        }
      }
      if (!stop) timer = window.setTimeout(tick, THUMB_MS);
    };
    void tick();
    return () => {
      stop = true;
      clearTimeout(timer);
    };
  }, [id, canvas]);
  return shown;
}

type MenuAt = { inst: Instance; shown: boolean | null; x: number; y: number; from: HTMLElement };

/** The game's name for a card, from the shared game cache. */
function GameName({ placeId }: { placeId: number | null }) {
  const g = useQuery({ queryKey: qk.game(placeId), queryFn: () => api.gameInfo(placeId!), enabled: placeId !== null }).data;
  return <span className="truncate">{placeId === null ? "No game yet" : g?.name ?? `Place ${placeId}`}</span>;
}

/** One running client: its live picture, who, where, how long and what it uses. */
function ClientCard({ inst: i, account: a, metrics: m, now, afkOn, onMenu }: {
  inst: Instance;
  account: AccountSummary | undefined;
  metrics: ClientMetrics | undefined;
  now: number;
  afkOn: boolean;
  onMenu: (at: MenuAt) => void;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const shown = useThumbnail(i.id, canvas);
  const name = a ? accountName(a) : `Account ${i.accountId}`;
  const [label, color] = STATES[i.state];
  const focus = () => api.instanceShow(i.id, "focus").catch(toastError);
  const badges: [string, string?][] = [
    ...(i.throttled ? [["Throttled", "Over the memory limit: held to the throttle's CPU limit"] as [string, string]] : []),
    ...(i.limits.cpuCap !== null ? [[`CPU ${i.limits.cpuCap}%`] as [string]] : []),
    ...(i.limits.eco ? [["Efficiency"] as [string]] : []),
    ...(afkOn && i.antiAfk ? [["Anti-AFK", i.afkAt !== null ? `Last key press ${uptime(i.afkAt, now)} ago` : "No key press yet"] as [string, string]] : []),
    ...(i.micro ? [["Micro"] as [string]] : []),
    ...(a?.fpsCap ? [[`${a.fpsCap} FPS`, "This account's FPS cap, set when its client starts"] as [string, string]] : []),

  ];
  return (
    <div
      role="group"
      tabIndex={0}
      aria-label={`${name}'s client, ${label}. Enter brings it to the front; the context menu key opens its actions.`}
      className="enter flex flex-col min-w-0 border border-line bg-panel outline-offset-2"
      title={`PID ${i.pid}`}
      onDoubleClick={focus}
      onKeyDown={(e) => {
        if (e.target !== e.currentTarget) return;
        if (e.key === "Enter") void focus();
        // From the keyboard (no pointer position): at the card's corner. Handled here so it
        // works whether or not the webview turns the key into a contextmenu event.
        if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
          e.preventDefault();
          const r = e.currentTarget.getBoundingClientRect();
          onMenu({ inst: i, shown, x: r.left + 12, y: r.top + 12, from: e.currentTarget });
        }
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        onMenu({ inst: i, shown, x: e.clientX, y: e.clientY, from: e.currentTarget });
      }}
    >
      <div className="relative aspect-[4/3] bg-bg border-b border-line overflow-hidden">
        <canvas ref={canvas} aria-hidden="true" className={`w-full h-full object-contain ${shown ? "" : "invisible"}`} />
        {!shown && (
          <span className="mono absolute inset-0 grid place-items-center text-xs text-dim">
            {shown === false ? (i.state === "starting" ? "Starting…" : "Minimized") : ""}
          </span>
        )}
      </div>
      <div className="flex flex-col gap-1.5 px-3 py-2.5 min-w-0">
        <span className="flex items-center gap-2 min-w-0">
          {a && <Avatar account={a} size={20} />}
          <span className="truncate font-medium">{name}</span>
          <span className="mono ml-auto flex-none text-xs text-dim">{uptime(i.startedAt, now)}</span>
        </span>
        <span className="mono flex items-baseline gap-1.5 text-xs min-w-0">
          <span aria-hidden="true" className={`w-1.5 h-1.5 flex-none ${i.state === "starting" || i.state === "joining" ? "pulse" : ""}`} style={{ background: color }} />
          <span className="flex-none">{label}</span>
          {i.note && <span className="text-orange truncate">· {i.note}</span>}
        </span>
        <span className="flex items-baseline gap-2 text-xs text-dim min-w-0">
          <GameName placeId={i.placeId} />
          <span className="mono ml-auto flex-none" style={{ fontVariantNumeric: "tabular-nums" }}>
            {m ? `${pct(m.cpuPct)} · ${mb(m.privateMb)}` : "—"}
          </span>
        </span>
        {badges.length > 0 && (
          <span className="flex flex-wrap gap-1">
            {badges.map(([b, t]) => (
              <span key={b} className={`tag ${b === "Throttled" ? "text-orange" : ""}`} title={t}>{b}</span>
            ))}
          </span>
        )}
      </div>
    </div>
  );
}

type Item =
  | { icon?: IconName; label: string; run: () => unknown; danger?: boolean; checked?: boolean }
  | { caps: true }
  | null;

/** A card's right-click menu: a popover, so Esc and a click outside close it; arrows move. */
function ClientMenu({ at, onClose, afkOn }: { at: MenuAt | null; onClose: () => void; afkOn: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: 0, top: 0 });

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || !at) return;
    el.showPopover();
    const r = el.getBoundingClientRect();
    setPos({ left: Math.max(4, Math.min(at.x, innerWidth - r.width - 4)), top: Math.max(4, Math.min(at.y, innerHeight - r.height - 4)) });
    el.querySelector<HTMLElement>("[role^=menuitem]")?.focus();
  }, [at]);

  useEffect(() => {
    const el = ref.current;
    const toggled = (e: Event) => {
      if ((e as ToggleEvent).newState !== "closed") return;
      // Focus goes back to the card unless the click went somewhere else.
      if (!document.activeElement || document.activeElement === document.body || el?.contains(document.activeElement)) at?.from.focus();
      onClose();
    };
    el?.addEventListener("toggle", toggled);
    return () => el?.removeEventListener("toggle", toggled);
  }, [onClose, at]);

  const i = at?.inst;
  const run = (f: () => unknown) => () => {
    ref.current?.hidePopover();
    void f();
  };
  const show = (what: WindowShow) => i && api.instanceShow(i.id, what).catch(toastError);
  const limits = (patch: Partial<Limits>) => i && api.instanceSetLimits(i.id, { ...i.limits, ...patch }).catch(toastError);
  const micro = () =>
    i &&
    api.instanceMicro(i.id, !i.micro).then(([w, h]) => {
      if (!i.micro && (w > 320 || h > 240)) toast(`Roblox kept this window at ${w} × ${h}; that is as small as it goes.`);
    }, toastError);
  const trim = () => i && api.instanceTrim(i.id).then(() => toast("Trimmed its memory. Windows pages it back in as the client needs it."), toastError);
  const close = (force: boolean) => i && api.instanceClose(i.id, force).catch(toastError);

  const items: Item[] = i
    ? [
        { icon: "external", label: "Focus", run: () => show("focus") },
        at.shown === false ? { label: "Restore", run: () => show("restore") } : { label: "Minimize", run: () => show("minimize") },
        { icon: "grid", label: i.micro ? "Normal size" : "Micro window (320 × 240)", run: micro },
        null,
        { icon: "refresh", label: "Trim memory", run: trim },
        { caps: true },
        { label: "Efficiency mode", checked: i.limits.eco, run: () => limits({ eco: !i.limits.eco }) },
        { label: afkOn ? "Anti-AFK" : "Anti-AFK (off in Settings)", checked: i.antiAfk, run: () => api.instanceSetAntiAfk(i.id, !i.antiAfk).catch(toastError) },
        null,
        { icon: "x", label: "Close", run: () => close(false) },
        { icon: "x", label: "Force close", run: () => close(true), danger: true },
      ]
    : [];

  const arrows = (e: ReactKeyboardEvent) => {
    const all = [...(ref.current?.querySelectorAll<HTMLElement>("[role^=menuitem]:not(:disabled)") ?? [])];
    const at = all.indexOf(document.activeElement as HTMLElement);
    const next = e.key === "ArrowDown" ? at + 1 : e.key === "ArrowUp" ? at - 1 : e.key === "Home" ? 0 : e.key === "End" ? all.length - 1 : null;
    if (next === null || !all.length) return;
    e.preventDefault();
    all[(next + all.length) % all.length].focus();
  };

  return (
    <div
      ref={ref}
      popover="auto"
      role="menu"
      aria-label="Client actions"
      className="menu"
      onKeyDown={arrows}
      style={{ position: "fixed", positionArea: "none", inset: "auto", left: pos.left, top: pos.top, margin: 0, maxHeight: "calc(100vh - 8px)" }}
    >
      {items.map((it, n) =>
        it === null ? (
          <div key={n} role="separator" className="h-px my-1 bg-line" />
        ) : "caps" in it ? (
          <div key="caps" role="group" aria-label="CPU limit" className="flex items-center gap-1 h-8 px-2.5">
            <span className="flex-1 text-dim">CPU limit</span>
            {CAPS.map((c) => (
              <button
                key={c}
                role="menuitemradio"
                aria-checked={(i?.limits.cpuCap ?? 0) === c}
                className="seg mono h-6 px-1.5 text-xs"
                aria-label={c ? `CPU limit ${c}%` : "No CPU limit"}
                onClick={run(() => limits({ cpuCap: c || null }))}
              >
                {c ? `${c}` : "Off"}
              </button>
            ))}
          </div>
        ) : (
          <button
            key={it.label}
            role={it.checked === undefined ? "menuitem" : "menuitemcheckbox"}
            aria-checked={it.checked}
            className={`menu-item ${it.danger ? "text-orange" : ""}`}
            onClick={run(it.run)}
          >
            {it.checked !== undefined ? (
              <Icon name="check" size={12} className={it.checked ? "" : "invisible"} />
            ) : it.icon ? (
              <Icon name={it.icon} size={12} />
            ) : (
              <span className="w-3" />
            )}
            {it.label}
          </button>
        ),
      )}
    </div>
  );
}

/** Every running client at a glance, with a picture of its window, and the window tools. */
export function DashboardView({ accounts, onLaunch }: { accounts: AccountSummary[]; onLaunch: () => void }) {
  const running = (useInstances().data ?? []).filter(isRunning);
  const metrics = useMetrics().data ?? [];
  const afkOn = useSettings().data?.antiAfk.enabled ?? false;
  const now = useNow(30_000);
  const [menu, setMenu] = useState<MenuAt | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);
  const [busy, setBusy] = useState(false);
  const byId = new Map(accounts.map((a) => [a.id, a]));
  const byInst = new Map(metrics.map((m) => [m.instanceId, m]));
  const inGame = running.filter((i) => i.state === "inGame").length;
  const cpu = metrics.some((m) => m.cpuPct !== null) ? metrics.reduce((n, m) => n + (m.cpuPct ?? 0), 0) : null;
  const mem = metrics.reduce((n, m) => n + m.privateMb, 0);

  const act = (f: () => Promise<unknown>) => () => {
    setBusy(true);
    f().catch(toastError).finally(() => setBusy(false));
  };
  const tile = (micro: boolean) =>
    act(() =>
      api.instancesTile(micro).then((n) => {
        if (n < running.length) toast(`Laid out ${plural(n, "client")}. Minimized ones and ones without a window yet were left as they are.`);
      }),
    );

  return (
    <main className="scroll flex-1 min-w-0 px-10 pt-6 pb-12">
      <div className="enter flex flex-col">
        <h1 className="text-xl font-semibold tracking-[-0.01em]">Dashboard</h1>
        <p className="text-dim mt-1">Every client RoALT runs. Double-click one to bring it to the front; right-click for more.</p>

        <div className="flex flex-wrap items-center gap-2 mt-5">
          <button className="btn btn-sm" disabled={busy || !running.length} onClick={tile(false)} title="Lay every client out on this monitor so all fit">
            <Icon name="grid" size={12} />
            Tile
          </button>
          <button className="btn btn-sm" disabled={busy || !running.length} onClick={tile(true)} title="Make every client 320 × 240 and lay them out on this monitor">
            Tile micro
          </button>
          <button className="btn btn-sm" disabled={busy || !running.length} onClick={act(() => api.instancesShowAll("minimize"))}>
            Minimize all
          </button>
          <button className="btn btn-sm" disabled={busy || !running.length} onClick={act(() => api.instancesShowAll("restore"))}>
            Restore all
          </button>
          <div className="w-40">
            <KillAllButton />
          </div>
          <span className="mono ml-auto text-xs text-dim" style={{ fontVariantNumeric: "tabular-nums" }}>
            {running.length} running · {inGame} in game · CPU {pct(cpu)} · {mb(mem)}
          </span>
        </div>

        {running.length === 0 ? (
          <div className="flex flex-col items-start gap-3 mt-8">
            <p className="text-dim">No clients are running. Clients you launch show here with a live picture of their window.</p>
            <button className="btn" onClick={onLaunch}>
              <Icon name="play" size={12} fill />
              Go to Launch
            </button>
          </div>
        ) : (
          <div className="grid gap-3 mt-5" style={{ gridTemplateColumns: "repeat(auto-fill, minmax(240px, 1fr))" }}>
            {running.map((i) => (
              <ClientCard key={i.id} inst={i} account={byId.get(i.accountId)} metrics={byInst.get(i.id)} now={now} afkOn={afkOn} onMenu={setMenu} />
            ))}
          </div>
        )}
        <p className="text-xs text-dim mt-6 max-w-[720px]">
          Pictures update every 2 seconds while RoALT is in front; a minimized client draws nothing. Micro window asks Roblox for a
          320 × 240 window; Roblox may keep it bigger. Anti-AFK skips minimized clients.
        </p>
      </div>
      <ClientMenu at={menu} onClose={closeMenu} afkOn={afkOn} />
    </main>
  );
}
