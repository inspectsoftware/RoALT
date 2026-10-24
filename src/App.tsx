import { useEffect, useRef, useState } from "react";
import { QueryClientProvider, useQueryClient } from "@tanstack/react-query";
import { api, asError, events, type Settings, type Theme, type VaultStatus } from "./api";
import { qk, queryClient, useAccounts, useGroups, useSetSettings, useSettings, useVault } from "./data";
import { AccountsView } from "./features/accounts/AccountsView";
import { AddAccountDialog } from "./features/accounts/AddAccountDialog";
import { GroupsNav } from "./features/accounts/GroupsNav";
import { SettingsView, SETTINGS_SECTIONS } from "./features/settings/SettingsView";
import { UnlockScreen, type LockReason } from "./features/vault/UnlockScreen";
import { Icon } from "./ui/Icon";
import { TitleBar } from "./ui/TitleBar";
import { friendly, toastError, Toaster } from "./ui/Toast";

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
    qc.removeQueries({ queryKey: qk.accounts });
  };

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
        onToggleTheme={s ? () => setSettings.mutate({ theme: dark ? "light" : "dark" }) : undefined}
        onLock={unlocked && v.mode === "master" ? lock : undefined}
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
  const accounts = useAccounts();
  const groups = useGroups();
  const [view, setView] = useState<"accounts" | "settings">("accounts");
  const [group, setGroup] = useState<number | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [adding, setAdding] = useState(false);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || document.querySelector("dialog[open]")) return;
      const k = e.key.toLowerCase();
      if (k === "n") {
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
  }, []);

  if (!accounts.data || !groups.data) return <div className="flex-1" />;

  const showAll = () => {
    setView("accounts");
    setGroup(null);
  };

  return (
    <div className="flex-1 flex min-h-0">
      <nav aria-label="Sections" className="w-52 flex-none flex flex-col px-2 py-3 bg-panel border-r border-line">
        <div className="flex flex-col gap-0.5">
          <button className="nav-item" aria-current={view === "accounts" ? "page" : undefined} onClick={showAll}>
            <Icon name="accounts" />
            <span className="flex-1">Accounts</span>
            <span className="mono text-xs text-dim">{accounts.data.length}</span>
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
        <div className="mono flex items-center gap-2 mt-3 p-3 border border-line text-[11px] text-dim">
          <span aria-hidden="true" className="w-1.5 h-1.5 flex-none bg-accent-text" />
          Vault · {vault.mode === "master" ? "master password" : "DPAPI"} · unlocked
        </div>
      </nav>

      {view === "accounts" ? (
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
        />
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

/** Applies the theme to <html>; "system" follows prefers-color-scheme live. Returns whether it is dark. */
function useTheme(theme: Theme | undefined) {
  const [dark, setDark] = useState(() => document.documentElement.dataset.theme !== "light");
  useEffect(() => {
    if (!theme) return;
    const mq = matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      const t = theme === "system" ? (mq.matches ? "dark" : "light") : theme;
      document.documentElement.dataset.theme = t;
      setDark(t === "dark");
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
