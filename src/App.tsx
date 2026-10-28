import { useEffect, useRef, useState } from "react";
import { QueryClientProvider, useQueryClient } from "@tanstack/react-query";
import { api, asError, events, type Favourite, type SameServer, type Settings, type Theme, type VaultStatus } from "./api";
import { qk, queryClient, useAccounts, useGroups, useInstances, useSetSettings, useSettings, useVault } from "./data";
import { AccountsView } from "./features/accounts/AccountsView";
import { AddAccountDialog } from "./features/accounts/AddAccountDialog";
import { DashboardView } from "./features/dashboard/DashboardView";
import { DiscordView } from "./features/discord/DiscordView";
import { GroupsNav } from "./features/accounts/GroupsNav";
import { plural } from "./features/accounts/shared";
import { FavouritesNav } from "./features/launch/Favourites";
import { lastLaunchAccount, LaunchView } from "./features/launch/LaunchView";
import { isRunning, KillAllButton, MultiInstanceSwitch, RunningPanel } from "./features/launch/Running";
import { draftFor, EMPTY_DRAFT, type Draft } from "./features/launch/target";
import { MonitorView } from "./features/monitor/MonitorView";
import { ServersView } from "./features/servers/ServersView";
import { SettingsView, SETTINGS_SECTIONS } from "./features/settings/SettingsView";
import { UnlockScreen, type LockReason } from "./features/vault/UnlockScreen";
import { Icon } from "./ui/Icon";
import { Divider, usePanelWidth } from "./ui/Resizer";
import { TitleBar } from "./ui/TitleBar";
import { friendly, toast, toastError, Toaster } from "./ui/Toast";

/** Last applied theme, read before first paint so a light-theme start does not flash dark. */
export const THEME_KEY = "roalt.theme";

export default function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <Root />
      <Toaster />
    </QueryClientProvider>
  );
}

function Root() {
  const qc = useQueryClient();
  const vault = useVault();
  const settings = useSettings();
  const setSettings = useSetSettings();
  const dark = useTheme(settings.data?.theme);
  const [lockReason, setLockReason] = useState<LockReason>("start");
  const manualLockAt = useRef(0);
  const v = vault.data;
  const s = settings.data;
  const unlocked = !!v && !v.locked;

  const markLocked = () => {
    qc.setQueryData<VaultStatus>(qk.vault, (old) => old && { ...old, locked: true });
    for (const queryKey of [qk.accounts, qk.presence, ["info"], qk.favourites, qk.join, qk.privateServers, qk.discord]) qc.removeQueries({ queryKey });
  };

  // Everything on screen asks again; the webview itself is never reloaded.
  const refresh = () => void qc.invalidateQueries();

  const lock = () => {
    manualLockAt.current = Date.now();
    setLockReason("manual");
    api.vaultLock().then(markLocked, toastError);
  };

  useEffect(() => {
    const subs = [
      events.onAccountsChanged(() => {
        for (const queryKey of [qk.accounts, qk.groups, qk.vault]) qc.invalidateQueries({ queryKey });
      }),
      events.onInstancesChanged(() => {
        for (const queryKey of [qk.instances, qk.multi, qk.crashes]) qc.invalidateQueries({ queryKey });
      }),
      events.onBulkProgress((p) => qc.setQueryData(qk.bulk, p)),
      events.onRobloxClient((c) => qc.setQueryData(qk.client, c)),
      // Errors stay until dismissed: the window may have been hidden in the tray.
      events.onNotice((n) => toast(n.message, n.error ? "error" : "ok", n.error)),
      events.onVaultLocked(() => {
        if (Date.now() - manualLockAt.current > 3000) setLockReason("idle");
        markLocked();
        qc.invalidateQueries({ queryKey: qk.vault });
      }),
    ];
    return () => subs.forEach((p) => p.then((un) => un()));
  }, [qc]);

  useActivity(unlocked);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const ctrl = e.ctrlKey || e.metaKey;
      const k = e.key.toLowerCase();
      if ((ctrl && k === "r") || e.key === "F5") e.preventDefault(); // never reload the webview
      if (e.key === "F5" && unlocked) refresh();
      if (ctrl && k === "l" && unlocked && v?.mode === "master") {
        e.preventDefault();
        lock();
      }
    };
    // The WebView2 menu (Reload, Inspect) has no place in the app; fields keep theirs.
    const onMenu = (e: MouseEvent) => {
      if (import.meta.env.PROD && !(e.target instanceof Element && e.target.closest("input, textarea, .selectable"))) e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("contextmenu", onMenu);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("contextmenu", onMenu);
    };
  });

  const failed = vault.error ?? settings.error;
  return (
    <div className="h-full flex flex-col">
      <TitleBar
        dark={dark}
        // Flips light and dark only: a named palette (Neon, Sakura, Bricks) is changed in Settings.
        onToggleTheme={s && ["dark", "light", "system"].includes(s.theme) ? () => setSettings.mutate({ theme: dark ? "light" : "dark" }) : undefined}
        onLock={unlocked && v.mode === "master" ? lock : undefined}
        onRefresh={unlocked ? refresh : undefined}
      />
      {failed ? (
        <div className="flex-1 grid place-items-center p-8">
          <div className="flex flex-col items-start gap-3 max-w-sm">
            <h1 className="text-[15px] font-semibold">RoALT couldn't start</h1>
            <p className="text-dim">{friendly(asError(failed))}</p>
            <button className="btn" onClick={() => qc.invalidateQueries()}>Try again</button>
          </div>
        </div>
      ) : !v || !s ? (
        <div className="flex-1" />
      ) : v.locked ? (
        <UnlockScreen status={v} reason={lockReason} />
      ) : (
        <Shell vault={v} settings={s} />
      )}
    </div>
  );
}

function Shell({ vault, settings }: { vault: VaultStatus; settings: Settings }) {
  const nav = usePanelWidth("nav", 208, 168, 320);
  const qc = useQueryClient();
  const accounts = useAccounts();
  const groups = useGroups();
  const running = useInstances().data?.filter(isRunning).length ?? 0;
  const [view, setView] = useState<"dashboard" | "accounts" | "launch" | "servers" | "monitor" | "discord" | "settings">("dashboard");
  const [group, setGroup] = useState<number | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  // The Launch view's accounts; the last one launched until something else is picked.
  const [picked, setPicked] = useState<number[]>(() => {
    const id = lastLaunchAccount();
    return id === null ? [] : [id];
  });
  const [sameServer, setSameServer] = useState<SameServer>("off");
  const [draft, setDraft] = useState<Draft>(EMPTY_DRAFT);
  // Kept here so a launch in flight survives leaving the view.
  const [launchStep, setLaunchStep] = useState<string | null>(null);
  // The game the Servers view shows.
  const [serversPlace, setServersPlace] = useState<number | null>(null);
  const [adding, setAdding] = useState(false);
  const known = useRef<Set<number> | null>(null);

  // A new account needs its headshot; the backend only asks Roblox for the missing ones.
  useEffect(() => {
    if (!accounts.data) return;
    const ids = new Set(accounts.data.map((a) => a.id));
    if (known.current && [...ids].some((id) => !known.current?.has(id))) qc.invalidateQueries({ queryKey: qk.headshots });
    known.current = ids;
  }, [accounts.data, qc]);

  // Opens on the Launch target's game until a game is loaded here.
  const openServers = () => {
    const p = draft.parsed?.text === draft.text ? draft.parsed.value : null;
    const t = p?.kind === "target" ? p.target : null;
    if (serversPlace === null && t && "placeId" in t) setServersPlace(t.placeId);
    setView("servers");
  };

  // Re-bound each render: Ctrl+4 needs the current draft. Alt is left alone (AltGr types characters).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey || document.querySelector("dialog[open]")) return;
      const k = e.key.toLowerCase();
      // By key position, so Ctrl+1 works on layouts where the digits need Shift.
      const views: Record<string, () => void> = {
        Digit1: () => setView("dashboard"),
        Digit2: () => setView("accounts"),
        Digit3: () => setView("launch"),
        Digit4: openServers,
        Digit5: () => setView("settings"),
        Digit6: () => setView("monitor"),
        Digit7: () => setView("discord"),
      };
      const go = views[e.code];
      if (go) {
        e.preventDefault();
        go();
      } else if (k === "n") {
        e.preventDefault();
        setAdding(true);
      } else if (k === "f") {
        e.preventDefault();
        setView("accounts");
        requestAnimationFrame(() => document.getElementById("account-filter")?.focus());
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const failed = accounts.error ?? groups.error;
  if (failed)
    return (
      <div className="flex-1 grid place-items-center p-8">
        <div className="flex flex-col items-start gap-3 max-w-sm">
          <p className="text-dim">{friendly(asError(failed))}</p>
          <button className="btn" onClick={() => { accounts.refetch(); groups.refetch(); }}>Try again</button>
        </div>
      </div>
    );
  if (!accounts.data || !groups.data) return <div className="flex-1" />;

  const showAll = () => {
    setView("accounts");
    setGroup(null);
  };
  /** Opens Launch with these accounts picked; ones that can't launch are left out. */
  const launchAs = (ids: number[]) => {
    const valid = ids.filter((id) => accounts.data?.some((a) => a.id === id && a.status === "valid"));
    if (!valid.length) return;
    if (valid.length < ids.length) toast(`Left out ${plural(ids.length - valid.length, "account")} that can't launch`);
    setPicked(valid);
    setView("launch");
  };
  const pickFavourite = (f: Favourite) => {
    setDraft(draftFor(f.target, f.id, f.name));
    const valid = f.accountIds.filter((id) => accounts.data?.some((a) => a.id === id && a.status === "valid"));
    if (valid.length) setPicked(valid);
    setView("launch");
  };
  return (
    <div className="flex-1 flex min-h-0">
      <nav aria-label="Sections" style={{ width: nav.width }} className="flex-none flex flex-col px-2 py-3 bg-panel border-r border-line">
        <div className="flex flex-col gap-0.5">
          <button className="nav-item" aria-current={view === "dashboard" ? "page" : undefined} onClick={() => setView("dashboard")}>
            <Icon name="grid" />
            <span className="flex-1">Dashboard</span>
            {running > 0 && (
              <span className="mono text-xs text-accent-text" title={`${running} running`}>
                {running}
              </span>
            )}
          </button>
          <button className="nav-item" aria-current={view === "accounts" ? "page" : undefined} onClick={showAll}>
            <Icon name="accounts" />
            <span className="flex-1">Accounts</span>
            <span className="mono text-xs text-dim">{accounts.data.length}</span>
          </button>
          <button className="nav-item" aria-current={view === "launch" ? "page" : undefined} onClick={() => setView("launch")}>
            <Icon name="play" fill />
            <span className="flex-1">Launch</span>
            {running > 0 && (
              <span className="mono text-xs text-accent-text" title={`${running} running`}>
                {running}
              </span>
            )}
          </button>
          <button className="nav-item" aria-current={view === "servers" ? "page" : undefined} onClick={openServers}>
            <Icon name="servers" />
            <span className="flex-1">Servers</span>
          </button>
          <button className="nav-item" aria-current={view === "monitor" ? "page" : undefined} onClick={() => setView("monitor")}>
            <Icon name="monitor" />
            <span className="flex-1">Monitor</span>
          </button>
          <button className="nav-item" aria-current={view === "discord" ? "page" : undefined} onClick={() => setView("discord")}>
            <Icon name="bell" />
            <span className="flex-1">Discord</span>
          </button>
          <button className="nav-item" aria-current={view === "settings" ? "page" : undefined} onClick={() => setView("settings")}>
            <Icon name="settings" />
            <span className="flex-1">Settings</span>
          </button>
          {view === "settings" && (
            <div className="enter flex flex-col pl-7 pt-1">
              {SETTINGS_SECTIONS.map(([id, label]) => (
                <button key={id} className="sub" onClick={() => document.getElementById(id)?.scrollIntoView({ block: "start" })}>
                  {label}
                </button>
              ))}
            </div>
          )}
        </div>
        <div className="scroll flex-1 min-h-0">
          <FavouritesNav accounts={accounts.data} active={view === "launch" ? draft.favId : null} onPick={pickFavourite} />
          <GroupsNav
            groups={groups.data}
            accounts={accounts.data}
            active={view === "accounts" ? group : null}
            onPick={(id) => {
              setGroup(id);
              setView("accounts");
            }}
          />
        </div>
        <div className="flex flex-col gap-2.5 mt-3 p-3 border border-line">
          <MultiInstanceSwitch />
          <KillAllButton />
          <div className="mono flex items-center gap-2 text-[11px] text-dim">
            <span aria-hidden="true" className="w-1.5 h-1.5 flex-none bg-accent-text" />
            Vault · {vault.mode === "master" ? "master password" : "DPAPI"} · unlocked
          </div>
        </div>
      </nav>
      <Divider panel={nav} side="left" label="Sidebar width" />

      {view === "dashboard" ? (
        <DashboardView accounts={accounts.data} onLaunch={() => setView("launch")} />
      ) : view === "accounts" ? (
        <AccountsView
          accounts={accounts.data}
          groups={groups.data}
          density={settings.density}
          vaultMode={vault.mode}
          clipboardSeconds={settings.clipboardClearSeconds}
          group={group}
          setGroup={setGroup}
          selected={selected}
          setSelected={setSelected}
          onAdd={() => setAdding(true)}
          onLaunch={launchAs}
        />
      ) : view === "launch" ? (
        <>
          <LaunchView
            accounts={accounts.data}
            picked={picked}
            setPicked={setPicked}
            sameServer={sameServer}
            setSameServer={setSameServer}
            draft={draft}
            setDraft={setDraft}
            step={launchStep}
            setStep={setLaunchStep}
          />
          <RunningPanel accounts={accounts.data} onPick={setPicked} />
        </>
      ) : view === "servers" ? (
        <ServersView
          accounts={accounts.data}
          picked={picked}
          setPicked={setPicked}
          place={serversPlace}
          setPlace={setServersPlace}
          onBatch={() => setView("launch")}
          step={launchStep}
          setStep={setLaunchStep}
        />
      ) : view === "monitor" ? (
        <MonitorView accounts={accounts.data} />
      ) : view === "discord" ? (
        <DiscordView settings={settings} />
      ) : (
        <SettingsView vault={vault} settings={settings} />
      )}

      <AddAccountDialog
        open={adding}
        onClose={() => setAdding(false)}
        onAdded={(a) => {
          showAll();
          setSelected(a.id);
        }}
      />
    </div>
  );
}

/** Palettes on a light background (the rest are dark). */
const LIGHT_THEMES = ["light", "sakura"];

/** Applies the theme to <html> (`data-theme`, one block per palette in theme.css); "system"
 * follows prefers-color-scheme live. Returns whether it is dark. */
function useTheme(theme: Theme | undefined) {
  const [dark, setDark] = useState(() => !LIGHT_THEMES.includes(document.documentElement.dataset.theme ?? ""));
  useEffect(() => {
    if (!theme) return;
    const mq = matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      const t = theme === "system" ? (mq.matches ? "dark" : "light") : theme;
      document.documentElement.dataset.theme = t;
      setDark(!LIGHT_THEMES.includes(t));

      try {
        localStorage.setItem(THEME_KEY, t);
      } catch {
        // Storage can be unavailable; the theme still applies.
      }
    };
    apply();
    mq.addEventListener("change", apply);
    return () => mq.removeEventListener("change", apply);
  }, [theme]);
  return dark;
}

/** Tells the backend about user input (auto-lock), at most once every 30 s. */
function useActivity(enabled: boolean) {
  useEffect(() => {
    if (!enabled) return;
    let last = 0;
    const ping = () => {
      const now = Date.now();
      if (now - last < 30_000) return;
      last = now;
      api.appActivity().catch(() => {});
    };
    const types = ["pointerdown", "pointermove", "keydown", "wheel"] as const;
    types.forEach((t) => window.addEventListener(t, ping, { passive: true, capture: true }));
    return () => types.forEach((t) => window.removeEventListener(t, ping, { capture: true }));
  }, [enabled]);
}
