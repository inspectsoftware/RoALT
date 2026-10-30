import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type AccountSummary, type ClientMetrics, type GameInfo, type Instance, type Limits, type WindowShow } from "../../api";
import { qk, queryClient, useInstances, useMetrics, useSetSettings, useSettings, useSystemUsage } from "../../data";
import { Segmented, Switch } from "../../ui/controls";
import { ConfirmDialog } from "../../ui/Dialog";
import { Icon, type IconName } from "../../ui/Icon";
import { toast, toastError } from "../../ui/Toast";
import { fmtList, t } from "../../i18n";
import { accountName, Avatar } from "../accounts/shared";
import { EndedPanel, isRunning, LimitsDialog, STATES, uptime, useNow } from "../launch/Running";
import { launched, launchFailed } from "../launch/LaunchView";
import { mb, pct } from "../monitor/MonitorView";
import { menuKeys } from "../../ui/menu";
import { usePreviewFps, type PreviewFps } from "./previewFps";

const CAPS = [0, 50, 25, 10, 5];
/** How often a hidden RoALT window looks again whether it is shown. */
const HIDDEN_MS = 500;

/** Draws the client's window on `canvas` `fps` times a second while the RoALT window is
 * visible, one picture in flight at a time (a slow capture lowers the rate, never queues);
 * true / false once it is known whether there is a picture (false: minimized or no window
 * yet), null before; `painted`: the canvas holds a picture, the last one while `shown` is false;
 * `parked`: it is off-screen, not minimized. */
function useThumbnail(id: string, canvas: React.RefObject<HTMLCanvasElement | null>, fps: PreviewFps) {
  const [shown, setShown] = useState<boolean | null>(null);
  const [painted, setPainted] = useState(false);
  const [parked, setParked] = useState(false);
  useEffect(() => {
    let stop = false;
    let timer: number | undefined;
    const every = 1000 / fps;
    // A card scrolled out of sight costs no pictures.
    let inView = true;
    const seen = new IntersectionObserver(([e]) => void (inView = e.isIntersecting));
    if (canvas.current) seen.observe(canvas.current);
    const tick = async () => {
      const started = performance.now();
      const el = canvas.current;
      const visible = document.visibilityState === "visible" && inView;
      if (el && visible) {
        try {
          const buf = await api.instanceThumbnail(id, Math.min(480, Math.max(160, Math.round(el.clientWidth * devicePixelRatio))));
          if (stop) return;
          if (buf.byteLength < 12) {
            setShown(false);
            setParked(false);
          } else {
            const head = new DataView(buf);
            const [w, h] = [head.getUint32(0, true), head.getUint32(4, true)];
            // Resizing clears the canvas and costs a reallocation: only when the size changes.
            if (el.width !== w) el.width = w;
            if (el.height !== h) el.height = h;
            el.getContext("2d")?.putImageData(new ImageData(new Uint8ClampedArray(buf, 12, w * h * 4), w, h), 0, 0);
            setShown(true);
            setPainted(true);
            setParked(head.getUint32(8, true) === 1);
          }
        } catch {
          // Ended or not started: the card goes with the next instances list.
        }
      }
      if (!stop) timer = window.setTimeout(tick, visible ? Math.max(0, every - (performance.now() - started)) : HIDDEN_MS);
    };
    void tick();
    return () => {
      stop = true;
      clearTimeout(timer);
      seen.disconnect();
    };
  }, [id, canvas, fps]);
  return { shown, painted, parked };
}

/** `beside`: the left edge of the key that opened it. The menu opens left of that, not over it,
 * where a second click would land on an item. */
type MenuAt = { inst: Instance; hidden: boolean; x: number; y: number; from: HTMLElement; beside?: number };

const closeKey = (i: Instance) => `${i.id}:${i.pid}`;
const capitalized = (s: string) => s.charAt(0).toLocaleUpperCase() + s.slice(1);

/** The game's name for a card, from the shared game cache. */
function GameName({ placeId }: { placeId: number | null }) {
  const g = useQuery({ queryKey: qk.game(placeId), queryFn: () => api.gameInfo(placeId!), enabled: placeId !== null }).data;
  return <span className="truncate">{placeId === null ? t("dashboard.noGameYet") : g?.name ?? t("dashboard.place", { id: placeId })}</span>;
}

/** One running client: its live picture, who, where, how long, what it uses, and its keys. */
function ClientCard({ inst: i, account: a, metrics: m, now, afkOn, fps, closing, onMenu }: {
  inst: Instance;
  account: AccountSummary | undefined;
  metrics: ClientMetrics | undefined;
  now: number;
  afkOn: boolean;
  fps: PreviewFps;
  closing: boolean;
  onMenu: (at: MenuAt) => void;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const { shown, painted, parked } = useThumbnail(i.id, canvas, fps);
  // No window to show or minimize yet.
  const noWindow = !shown && i.state === "starting";
  const hidden = !noWindow && (shown === false || parked);
  const name = a ? accountName(a) : t("dashboard.account", { id: i.accountId });
  const rejoining = i.relaunching && i.state === "disconnected";
  const [label, color] = closing ? [t("launch.running.closing"), "var(--dim)"] : rejoining ? [t("launch.state.rejoining"), "var(--orange)"] : STATES[i.state];
  const moving = i.state === "starting" || i.state === "joining" || rejoining || closing;
  const focus = () => api.instanceShow(i.id, "focus").catch(toastError);
  const throttled = t("dashboard.throttled");
  const badges: [string, string?][] = [
    ...(i.throttled ? [[throttled, t("dashboard.throttledHint")] as [string, string]] : []),
    ...(i.limits.cpuCap !== null ? [[t("dashboard.cpuPct", { pct: i.limits.cpuCap })] as [string]] : []),
    ...(i.limits.eco ? [[t("dashboard.efficiency")] as [string]] : []),
    ...(i.limits.trimEveryMin !== null ? [[capitalized(t("launch.limits.trimItem"))] as [string]] : []),
    ...(i.limits.affinity !== null ? [[capitalized(t("launch.limits.coresItem"))] as [string]] : []),
    ...(afkOn && i.antiAfk ? [[t("dashboard.antiAfk"), i.afkAt !== null ? t("dashboard.lastKeyPress", { time: uptime(i.afkAt, now) }) : t("dashboard.noKeyPress")] as [string, string]] : []),
    ...(i.micro ? [[t("dashboard.micro")] as [string]] : []),
    ...(i.fpsCap ? [[t("dashboard.fps", { fps: i.fpsCap }), t("dashboard.launchFpsCap")] as [string, string]]
      : a?.fpsCap ? [[t("dashboard.fps", { fps: a.fpsCap }), t("dashboard.accountFpsCap")] as [string, string]] : []),
  ];
  return (
    <div
      role="group"
      tabIndex={0}
      aria-label={t("dashboard.cardLabel", { name, state: label })}
      className="enter panel relative flex flex-col gap-3 p-3.5 min-w-0 outline-offset-2"
      title={`PID ${i.pid}`}
      onDoubleClick={(e) => {
        if (!(e.target as Element).closest("button")) void focus();
      }}
      onKeyDown={(e) => {
        if (e.target !== e.currentTarget) return;
        if (e.key === "Enter") void focus();
        // From the keyboard (no pointer position): at the card's corner. Handled here so it
        // works whether or not the webview turns the key into a contextmenu event.
        if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
          e.preventDefault();
          const r = e.currentTarget.getBoundingClientRect();
          onMenu({ inst: i, hidden, x: r.left + 12, y: r.top + 12, from: e.currentTarget });
        }
      }}
      onContextMenu={(e) => {
        e.preventDefault();
        onMenu({ inst: i, hidden, x: e.clientX, y: e.clientY, from: e.currentTarget });
      }}
    >
      <div className="bezel">
        <div className="relative aspect-video overflow-hidden rounded-[9px]">
          <canvas ref={canvas} aria-hidden="true" className={`w-full h-full object-contain ${shown || painted ? "" : "invisible"} ${shown === false ? "opacity-50" : ""}`} />
          {!shown && !painted && (
            <span className="absolute inset-0 grid place-items-center text-xs text-dim">
              {shown === false ? (i.state === "starting" ? t("dashboard.starting") : t("dashboard.minimized")) : ""}
            </span>
          )}
        </div>
        {shown === false && painted && <span className="badge" title={t("dashboard.lastPictureHint")}>{t("dashboard.minimized")}</span>}
        {parked && <span className="badge" title={t("settings.parkMinimizedHint")}>{t("dashboard.parked")}</span>}
      </div>
      <div className="flex items-center gap-3 min-w-0">
        {a && <Avatar account={a} size={36} />}
        <div className="flex-1 min-w-0 flex flex-col">
          <span className="font-medium truncate">{name}</span>
          <span className="flex gap-1 text-[13px] text-dim min-w-0">
            <GameName placeId={i.placeId} />
            <span className="mono flex-none">· {uptime(i.startedAt, now)}</span>
          </span>
        </div>
        <span className="state flex-none text-[13px]" style={{ color }}>
          <span aria-hidden="true" className={`led ${moving ? "pulse" : ""}`} />
          {label}
        </span>
      </div>
      {i.note && !rejoining && <span className="text-[13px] text-orange">{i.note}</span>}
      <div className="flex items-center justify-between gap-2 text-xs text-dim min-w-0">
        <span className="flex flex-wrap gap-1 min-w-0">
          {badges.map(([b, tip]) => (
            <span key={b} className={`tag ${b === throttled ? "text-orange" : ""}`} title={tip}>{b}</span>
          ))}
        </span>
        <span className="mono flex-none ml-auto" style={{ fontVariantNumeric: "tabular-nums" }}>
          {m ? `${pct(m.cpuPct)} · ${mb(m.privateMb)}` : "—"}
        </span>
      </div>
      <div className="flex gap-2.5">
        <button className="btn flex-1 min-w-0 px-2 py-1 whitespace-normal leading-tight" disabled={noWindow} onClick={focus}>
          {t("home.show")}
        </button>
        {i.state === "disconnected" && !i.relaunching && a ? (
          // Nothing else brings it back with auto-rejoin off.
          <button className="btn btn-primary flex-1 min-w-0 px-2 py-1 whitespace-normal leading-tight" onClick={() => api.launch(a.id, i.target, undefined, i.fpsCap).then((info) => launched(a, info), launchFailed)}>
            {t("home.playAgain")}
          </button>
        ) : (
          <button className="btn flex-1 min-w-0 px-2 py-1 whitespace-normal leading-tight" disabled={noWindow} onClick={() => api.instanceShow(i.id, hidden ? "restore" : "minimize").catch(toastError)}>
            {hidden ? t("dashboard.restore") : t("dashboard.minimize")}
          </button>
        )}
        <button
          className="btn btn-icon"
          aria-label={t("home.moreFor", { name })}
          title={t("dashboard.clientActions")}
          aria-haspopup="menu"
          onClick={(e) => {
            const r = e.currentTarget.getBoundingClientRect();
            onMenu({ inst: i, hidden, x: r.left, y: r.top - 8, from: e.currentTarget, beside: r.left });
          }}
        >
          <Icon name="dots" />
        </button>
      </div>
    </div>
  );
}

type Item =
  | { icon?: IconName; label: string; run: () => unknown; danger?: boolean; checked?: boolean; disabled?: boolean }
  | { caps: true }
  | null;

/** A card's right-click menu: a popover, so Esc and a click outside close it; arrows move. */
function ClientMenu({ at, onClose, afkOn, closing, onLimits, onCloseClient }: {
  at: MenuAt | null;
  onClose: () => void;
  afkOn: boolean;
  closing: ReadonlySet<string>;
  onLimits: (id: string) => void;
  onCloseClient: (i: Instance, force: boolean) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: 0, top: 0 });

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || !at) return;
    el.showPopover();
    const r = el.getBoundingClientRect();
    setPos({
      left: at.beside === undefined ? Math.max(4, Math.min(at.x, innerWidth - r.width - 4)) : Math.max(4, at.beside - r.width - 6),
      top: Math.max(4, Math.min(at.y, innerHeight - r.height - 4)),
    });
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
  const settings = useSettings().data;
  const tiny = { w: settings?.microWidth ?? 320, h: settings?.microHeight ?? 240 };
  const run = (f: () => unknown) => () => {
    ref.current?.hidePopover();
    void f();
  };
  const show = (what: WindowShow) => i && api.instanceShow(i.id, what).catch(toastError);
  const limits = (patch: Partial<Limits>) => i && api.instanceSetLimits(i.id, { ...i.limits, ...patch }).catch(toastError);
  const micro = () =>
    i &&
    api.instanceMicro(i.id, !i.micro).then(([w, h]) => {
      if (!i.micro && (w > tiny.w || h > tiny.h)) toast(t("dashboard.microKept", { w, h }));
    }, toastError);
  const trim = () => i && api.instanceTrim(i.id).then(() => toast(t("dashboard.trimmed")), toastError);

  const items: Item[] = i
    ? [
        { icon: "external", label: t("dashboard.focus"), run: () => show("focus") },
        at.hidden ? { label: t("dashboard.restore"), run: () => show("restore") } : { label: t("dashboard.minimize"), run: () => show("minimize") },
        { icon: "grid", label: i.micro ? t("dashboard.normalSize") : t("dashboard.microWindow", tiny), run: micro },
        null,
        { icon: "refresh", label: t("dashboard.trimMemory"), run: trim },
        { caps: true },
        { label: t("dashboard.efficiencyMode"), checked: i.limits.eco, run: () => limits({ eco: !i.limits.eco }) },
        { label: afkOn ? t("dashboard.antiAfk") : t("dashboard.antiAfkOff"), checked: i.antiAfk, run: () => api.instanceSetAntiAfk(i.id, !i.antiAfk).catch(toastError) },
        { icon: "sliders", label: t("home.limits"), run: () => onLimits(i.id) },
        null,
        { icon: "x", label: t("common.close"), run: () => onCloseClient(i, false), disabled: closing.has(closeKey(i)) },
        { icon: "x", label: t("dashboard.forceClose"), run: () => onCloseClient(i, true), danger: true },
      ]
    : [];

  return (
    <div
      ref={ref}
      popover="auto"
      role="menu"
      aria-label={t("dashboard.clientActions")}
      className="menu"
      onKeyDown={menuKeys}
      style={{ position: "fixed", positionArea: "none", inset: "auto", left: pos.left, top: pos.top, margin: 0, maxHeight: "calc(100vh - 8px)" }}
    >
      {items.map((it, n) =>
        it === null ? (
          <div key={n} role="separator" className="h-px my-1 bg-line" />
        ) : "caps" in it ? (
          <div key="caps" role="group" aria-label={t("dashboard.cpuLimit")} className="flex items-center gap-1 min-h-10 px-2.5">
            <span className="flex-1 text-dim">{t("dashboard.cpuLimit")}</span>
            {CAPS.map((c) => (
              <button
                key={c}
                role="menuitemradio"
                aria-checked={(i?.limits.cpuCap ?? 0) === c}
                className="seg mono min-h-7 px-1.5 text-xs"
                aria-label={c ? t("dashboard.cpuLimitPct", { pct: c }) : t("dashboard.noCpuLimit")}
                onClick={run(() => limits({ cpuCap: c || null }))}
              >
                {c ? `${c}` : t("common.off")}
              </button>
            ))}
          </div>
        ) : (
          <button
            key={it.label}
            role={it.checked === undefined ? "menuitem" : "menuitemcheckbox"}
            aria-checked={it.checked}
            className={`menu-item ${it.danger ? "text-orange" : ""}`}
            disabled={it.disabled}
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

/** A share of something, as a sunk dial. Null: Windows doesn't say. */
function Dial({ value, label, hint, text }: { value: number | null; label: string; hint: string; text?: string }) {
  const circumference = 188.5;
  const shown = Math.max(0, Math.min(100, value ?? 0));
  return (
    <div className="flex-[1_1_170px] flex items-center gap-3.5 min-w-0">
      <div className="dial" style={{ color: "var(--accent-text)" }}>
        <svg viewBox="0 0 76 76" aria-hidden="true">
          {value !== null && <circle cx="38" cy="38" r="30" strokeDasharray={`${(shown / 100) * circumference} ${circumference}`} />}
        </svg>
        <span className="mono relative text-[15px] font-medium text-text">{text ?? (value === null ? "—" : `${Math.round(shown)}%`)}</span>
      </div>
      <div className="min-w-0">
        <p className="font-medium">{label}</p>
        <p className="text-[13px] text-dim">{hint}</p>
      </div>
    </div>
  );
}

/** How the whole PC is doing, in words and dials. */
function PcPanel({ metrics }: { metrics: ClientMetrics[] }) {
  const sys = useSystemUsage(true).data ?? null;
  const memPct = sys ? (sys.memUsedMb / Math.max(1, sys.memTotalMb)) * 100 : null;
  const gpu = metrics.some((m) => m.gpuPct !== null) ? metrics.reduce((n, m) => n + (m.gpuPct ?? 0), 0) : null;
  const worst = Math.max(sys?.cpuPct ?? 0, memPct ?? 0, gpu ?? 0);
  const [verdict, color] = worst < 60
    ? [t("home.plenty"), "var(--accent-text)"]
    : worst < 85
      ? [t("home.busy"), "var(--orange)"]
      : [t("home.flatOut"), "var(--err)"];
  return (
    <section aria-labelledby="pc-title" className="panel flex flex-wrap items-center gap-x-8 gap-y-5 p-5">
      <div className="flex-[1_1_170px] min-w-0">
        <h2 id="pc-title" className="section-title">{t("home.yourPc")}</h2>
        <p className="state mt-2" style={{ color }}>
          <span aria-hidden="true" className="led" />
          {verdict}
        </p>
      </div>
      <Dial value={sys?.cpuPct ?? null} label={t("home.cpu")} hint={t("home.wholePc")} />
      <Dial value={memPct} label={t("home.memory")} hint={sys ? t("home.memOf", { used: mb(sys.memUsedMb), total: mb(sys.memTotalMb) }) : t("home.wholePc")} />
      <Dial value={gpu} label={t("home.gpu")} hint={t("home.allClients")} />
    </section>
  );
}

/** Anti-AFK where it's seen, running clients or not; how often it presses stays in Settings. */
function AfkPanel() {
  const afk = useSettings().data?.antiAfk;
  const set = useSetSettings();
  if (!afk) return null;
  return (
    <section aria-labelledby="afk-title" className="panel flex flex-wrap items-center gap-x-5 gap-y-3 px-5 py-4">
      <div className="flex-[1_1_320px] min-w-0">
        <h2 id="afk-title" className="section-title">{t("settings.antiAfk")}</h2>
        <p className="text-dim text-[13px] mt-1">{afk.enabled ? (afk.mode === "focus" ? t("settings.antiAfkFocusHint") : t("settings.antiAfkBackgroundHint")) : t("settings.antiAfkHint")}</p>
      </div>
      {afk.enabled && (
        <Segmented
          label={t("settings.antiAfkMode")}
          value={afk.mode}
          options={[
            { value: "focus", label: t("settings.antiAfkFocus") },
            { value: "background", label: t("settings.inBackground") },
          ]}
          onChange={(mode) => set.mutate({ antiAfk: { ...afk, mode } })}
        />
      )}
      <Switch label={t("settings.antiAfk")} checked={afk.enabled} onChange={(enabled) => set.mutate({ antiAfk: { ...afk, enabled } })} />
    </section>
  );
}

/** What a client's relaunch is waiting for, with the ways out. */
function Trouble({ inst: i, account: a }: { inst: Instance; account: AccountSummary | undefined }) {
  const name = a ? accountName(a) : t("dashboard.account", { id: i.accountId });
  return (
    <section role="status" className="enter panel flex flex-wrap items-center gap-x-5 gap-y-3 px-5 py-4">
      <span aria-hidden="true" className="led pulse" style={{ color: "var(--orange)" }} />
      <div className="flex-[1_1_320px] min-w-0">
        <p className="font-medium">{t("home.lost", { name })}</p>
        <p className="text-dim text-[13px]">{t("home.retryLine", { why: i.relaunchProblem ?? "" })}</p>
      </div>
      <div className="flex gap-2.5">
        <button className="btn" onClick={() => api.instancesRetryRelaunch().catch(toastError)}>{t("home.tryNow")}</button>
        <button className="btn btn-ghost" onClick={() => api.instanceStopRelaunch(i.id).catch(toastError)}>{t("home.stopTrying")}</button>
      </div>
    </section>
  );
}

/** Three steps to a first game, or what to do while nothing runs. */
function Welcome({ hasAccounts, signedOut, onAdd, onImport, onPlay, onAccounts }: { hasAccounts: boolean; signedOut: boolean; onAdd: () => void; onImport: () => void; onPlay: () => void; onAccounts: () => void }) {
  return (
    <>
      <header className="max-w-[640px]">
        <h1 className="page-title">{signedOut ? t("home.signedOutTitle") : hasAccounts ? t("home.idle") : t("home.welcome")}</h1>
        <p className="page-sub">{signedOut ? t("home.signedOutLead") : hasAccounts ? t("home.idleLead") : t("home.welcomeLead")}</p>
        {signedOut && <button className="btn btn-primary mt-3" onClick={onAccounts}>{t("app.nav.accounts")}</button>}
      </header>
      <section className="grid gap-6" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(min(250px, 100%), 1fr))" }}>
        <article className="panel flex flex-col gap-3.5 p-5">
          <span className={`num ${hasAccounts ? "num-done" : ""}`}>{hasAccounts ? <Icon name="check" size={18} /> : "1"}</span>
          <h2 className="section-title">{t("home.step1")}</h2>
          <p className="flex-1 text-dim">{t("home.step1Text")}</p>
          <button className={`btn ${hasAccounts ? "" : "btn-primary"}`} onClick={onAdd}>
            <Icon name="plus" size={14} />
            {hasAccounts ? t("home.addAnother") : t("accounts.add.title")}
          </button>
        </article>
        <article className="panel flex flex-col gap-3.5 p-5">
          <span className="num">2</span>
          <h2 className="section-title">{t("home.step2")}</h2>
          <p className="flex-1 text-dim">{t("home.step2Text")}</p>
          <button className={`btn ${hasAccounts && !signedOut ? "btn-primary" : ""}`} disabled={!hasAccounts || signedOut} onClick={onPlay}>{t("home.chooseGame")}</button>
        </article>
        <article className="panel flex flex-col gap-3.5 p-5">
          <span className="num">3</span>
          <h2 className="section-title">{t("home.step3")}</h2>
          <p className="flex-1 text-dim">{t("home.step3Text")}</p>
          <p className="min-h-11 flex items-center text-[13px] text-dim">{t("home.noLimit")}</p>
        </article>
      </section>
      {!hasAccounts && (
        <p className="flex flex-wrap items-center gap-x-1 text-dim">
          {t("home.importLead")}
          <button className="btn btn-ghost px-2" onClick={onImport}>{t("home.import")}</button>
        </p>
      )}
    </>
  );
}

/** Every running client at a glance, with a picture of its window, and the window tools; with
 * nothing running, the way to the first game. */
export function DashboardView({ accounts, onLaunch, onAdd, onImport, onAccounts }: { accounts: AccountSummary[]; onLaunch: () => void; onAdd: () => void; onImport: () => void; onAccounts: () => void }) {
  const setSettings = useSetSettings();
  const qc = useQueryClient();
  const list = useInstances().data ?? [];
  const running = list.filter(isRunning);
  const metrics = useMetrics().data ?? [];
  const settings = useSettings().data;
  const afkOn = settings?.antiAfk.enabled ?? false;
  const now = useNow(30_000);
  const [menu, setMenu] = useState<MenuAt | null>(null);
  const closeMenu = useCallback(() => setMenu(null), []);
  // The key that opened the menu closes it again: the click already dismissed the popover.
  const openMenu = (at: MenuAt) => setMenu((m) => (at.beside !== undefined && m?.from === at.from ? null : at));
  const [limitsFor, setLimitsFor] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmClose, setConfirmClose] = useState(false);
  const [closing, setClosing] = useState(false);
  const [closingOne, setClosingOne] = useState<ReadonlySet<string>>(new Set());
  const [fps] = usePreviewFps();
  const byId = new Map(accounts.map((a) => [a.id, a]));
  const byInst = new Map(metrics.map((m) => [m.instanceId, m]));
  const inGame = running.filter((i) => i.state === "inGame").length;
  const troubles = list.filter((i) => i.relaunching && i.relaunchProblem);
  const coming = running.some((i) => i.state === "starting" || i.state === "joining" || (i.relaunching && i.state === "disconnected"));
  const limited = list.find((i) => i.id === limitsFor);
  const limitedAccount = limited && byId.get(limited.accountId);
  const limitedName = limited ? (limitedAccount ? accountName(limitedAccount) : t("dashboard.account", { id: limited.accountId })) : "";

  const games = [...new Set(running.flatMap((i) => (i.state !== "inGame" || i.placeId === null ? [] : [i.placeId])))].map((id) => qc.getQueryData<GameInfo>(qk.game(id))?.name).filter((n): n is string => !!n);

  const act = (f: () => Promise<unknown>) => () => {
    setBusy(true);
    f().catch(toastError).finally(() => setBusy(false));
  };
  const tile = (micro: boolean) =>
    act(() =>
      api.instancesTile(micro).then((n) => {
        if (n < running.length) toast(t("dashboard.laidOut", { count: n }));
      }),
    );
  /** Keyed by pid too: a close that reached only a bootstrapper ends with the client adopted
   * under a new pid, which can then be closed again. */
  const closeOne = (i: Instance, force: boolean) => {
    const key = closeKey(i);
    setClosingOne((s) => new Set(s).add(key));
    api.instanceClose(i.id, force).catch((e) => {
      setClosingOne((s) => {
        const next = new Set(s);
        next.delete(key);
        return next;
      });
      toastError(e);
    });
  };
  const closeAll = () => {
    setClosing(true);
    api
      .instancesCloseAll(false)
      .catch(toastError)
      .finally(() => {
        setClosing(false);
        setConfirmClose(false);
      });
  };

  return (
    <main className="scroll flex-1 min-w-0 px-8 pt-5 pb-12">
      <div className="enter flex flex-col gap-6">
        {running.length === 0 ? (
          <Welcome hasAccounts={accounts.length > 0} signedOut={accounts.length > 0 && accounts.every((a) => a.status !== "valid")} onAdd={onAdd} onImport={onImport} onPlay={onLaunch} onAccounts={onAccounts} />
        ) : (
          <>
            <header className="flex flex-wrap items-center justify-between gap-4">
              <div className="min-w-0">
                <h1 className="page-title">{inGame ? t("home.playing", { count: inGame }) : coming ? t("home.gettingIn") : t("home.noneInGame")}</h1>
                <p className="page-sub">
                  {games.length > 0 && <>{t("home.in", { games: fmtList(games) })} </>}
                  {settings?.autoRejoin ? t("home.autoRejoinOn") : t("home.autoRejoinOff")}
                  {settings && !settings.autoRejoin && (
                    <button className="btn btn-ghost btn-sm ml-1" onClick={() => setSettings.mutate({ autoRejoin: true })}>{t("launch.multi.turnOn")}</button>
                  )}
                </p>
              </div>
              <div className="flex flex-wrap items-center gap-3">
                <button className="btn" disabled={busy} onClick={tile(false)} title={t("dashboard.tileHint")}>{t("dashboard.tile")}</button>
                <button className="btn" disabled={busy} onClick={act(() => api.instancesShowAll("minimize"))}>{t("dashboard.minimizeAll")}</button>
                <button className="btn btn-icon" popoverTarget="home-more" style={{ anchorName: "--home-more" }} aria-label={t("home.more")} title={t("home.more")}>
                  <Icon name="dots" />
                </button>
                <div id="home-more" popover="auto" className="menu" onKeyDown={menuKeys} style={{ positionAnchor: "--home-more", positionArea: "block-end span-inline-start", margin: "6px 0 0" }}>
                  <button className="menu-item" popoverTarget="home-more" popoverTargetAction="hide" disabled={busy} onClick={act(() => api.instancesShowAll("restore"))}>{t("dashboard.restoreAll")}</button>
                  <button className="menu-item" popoverTarget="home-more" popoverTargetAction="hide" disabled={busy} title={t("dashboard.tileMicroHint", { w: settings?.microWidth ?? 320, h: settings?.microHeight ?? 240 })} onClick={tile(true)}>{t("dashboard.tileMicro")}</button>
                  <button
                    className="menu-item"
                    popoverTarget="home-more"
                    popoverTargetAction="hide"
                    title={t("dashboard.refreshHint")}
                    onClick={() => {
                      for (const queryKey of [qk.instances, qk.metrics, qk.multi]) void queryClient.invalidateQueries({ queryKey });
                    }}
                  >
                    {t("dashboard.refresh")}
                  </button>
                  <div role="separator" className="h-px my-1 bg-line" />
                  <button className="menu-item text-orange" popoverTarget="home-more" popoverTargetAction="hide" onClick={() => setConfirmClose(true)}>{t("launch.running.closeAll")}</button>
                </div>
                <button className="btn btn-primary" onClick={onLaunch}>
                  <Icon name="play" size={12} fill />
                  {t("home.playMore")}
                </button>
              </div>
            </header>
            {troubles.map((i) => <Trouble key={i.id} inst={i} account={byId.get(i.accountId)} />)}
            <PcPanel metrics={metrics} />
            <AfkPanel />
            <section aria-label={t("launch.running.title")} className="grid gap-6" style={{ gridTemplateColumns: "repeat(auto-fill, minmax(min(280px, 100%), 1fr))" }}>
              {running.map((i) => (
                <ClientCard key={i.id} inst={i} account={byId.get(i.accountId)} metrics={byInst.get(i.id)} now={now} afkOn={afkOn} fps={fps} closing={closingOne.has(closeKey(i))} onMenu={openMenu} />
              ))}
            </section>
          </>
        )}
        {running.length === 0 && accounts.length > 0 && <AfkPanel />}
        {running.length === 0 && troubles.map((i) => <Trouble key={i.id} inst={i} account={byId.get(i.accountId)} />)}
        <EndedPanel accounts={accounts} />
      </div>
      <ClientMenu at={menu} onClose={closeMenu} afkOn={afkOn} closing={closingOne} onLimits={setLimitsFor} onCloseClient={closeOne} />
      <LimitsDialog instanceId={limitsFor} name={limitedName} onClose={() => setLimitsFor(null)} />
      <ConfirmDialog
        open={confirmClose}
        onClose={() => !closing && setConfirmClose(false)}
        onConfirm={closeAll}
        busy={closing}
        title={t("launch.running.closeAllTitle")}
        text={t("launch.running.closeAllText", { count: running.length })}
        confirmLabel={t("launch.running.closeAll")}
      />
    </main>
  );
}
