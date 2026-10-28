import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { open as openFile, save as saveFile } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, asError, events, type BandwidthStatus, type ConflictPolicy, type ImportReport, type InstallProgress, type RobloxClient, type Settings, type VaultStatus } from "../../api";
import { qk, useMultiStatus, useSetSettings } from "../../data";
import { NumberField, Segmented, Switch, TextField } from "../../ui/controls";
import { ConfirmDialog, Dialog } from "../../ui/Dialog";
import { Icon } from "../../ui/Icon";
import { friendly, toast, toastError } from "../../ui/Toast";
import { plural } from "../accounts/shared";
import { useMultiToggle } from "../launch/Running";
import { PasswordDialog } from "../vault/PasswordDialog";
import { unhideUpdate } from "./UpdateBanner";

/** Accent presets: value ("" is the theme's own lime) and name. Any other colour comes from the picker. */
const ACCENTS: [string, string][] = [
  ["", "Lime"],
  ["#ff4fd8", "Neon"],
  ["#f48fb8", "Sakura"],
  ["#e2603f", "Brick"],
  ["#ff9a3c", "Orange"],
  ["#ffd23f", "Gold"],
  ["#2ee6c5", "Mint"],
  ["#4f8cff", "Blue"],
  ["#a66bff", "Violet"],
];
const PICKER_FILTER = [{ name: "Images", extensions: ["png", "jpg", "jpeg", "webp", "gif"] }];

export const SETTINGS_SECTIONS = [
  ["general", "General"],
  ["launching", "Launching"],
  ["performance", "Performance"],
  ["automation", "Automation"],
  ["security", "Security"],
  ["data", "Data"],
  ["shortcuts", "Shortcuts"],
  ["about", "About"],
] as const;

const SHORTCUTS = [
  ["Go to Dashboard", "Ctrl 1"],
  ["Go to Accounts", "Ctrl 2"],
  ["Go to Launch", "Ctrl 3"],
  ["Go to Servers", "Ctrl 4"],
  ["Go to Settings", "Ctrl 5"],
  ["Go to Monitor", "Ctrl 6"],
  ["Go to Discord", "Ctrl 7"],
  ["Add account", "Ctrl N"],
  ["Filter accounts", "Ctrl F"],
  ["Select all shown", "Ctrl A"],
  ["Validate selected, or all", "Ctrl R"],
  ["Remove selected", "Delete"],
  ["Launch (Launch view)", "Ctrl Enter"],
  ["Refresh what's shown", "F5"],
  ["Lock vault (master password)", "Ctrl L"],
  ["Close dialog, clear selection", "Esc"],
];

const BACKUP_FILTER = [{ name: "RoALT backup", extensions: ["roalt"] }];
const RELEASES = "https://github.com/inspectsoftware/RoALT/releases";
const cancelled = () => ({ kind: "cancelled", message: "" });

type VaultDialog = "set" | "change" | "remove" | "export";

export function SettingsView({ vault, settings }: { vault: VaultStatus; settings: Settings }) {
  const qc = useQueryClient();
  const set = useSetSettings();
  const master = vault.mode === "master";
  const [dlg, setDlg] = useState<VaultDialog | null>(null);
  const [importPath, setImportPath] = useState("");
  const [importOpen, setImportOpen] = useState(false);
  const multi = useMultiStatus().data;
  const multiToggle = useMultiToggle((st) => st.message && toast(st.message, "error"));
  // Asked again each time Settings opens: Roblox may have been installed meanwhile.
  const client = useQuery({ queryKey: qk.client, queryFn: api.robloxClient, staleTime: 0 }).data;
  const dataDir = useQuery({ queryKey: qk.dataDir, queryFn: api.dataDirInfo }).data;
  const [movePath, setMovePath] = useState("");
  const [moveOpen, setMoveOpen] = useState(false);
  const [moving, setMoving] = useState(false);

  const setMode = async (mode: "dpapi" | "master", current?: string, next?: string, done?: string) => {
    await api.vaultSetMode(mode, current || undefined, next || undefined);
    qc.invalidateQueries({ queryKey: qk.vault });
    if (done) toast(done);
  };

  async function pickImport() {
    try {
      const path = await openFile({ multiple: false, directory: false, filters: BACKUP_FILTER });
      if (path) {
        setImportPath(path);
        setImportOpen(true);
      }
    } catch (e) {
      toastError(e);
    }
  }

  async function pickDataDir() {
    try {
      const path = await openFile({ directory: true, multiple: false, title: "Choose a folder for RoALT's data" });
      if (path) {
        setMovePath(path);
        setMoveOpen(true);
      }
    } catch (e) {
      toastError(e);
    }
  }

  async function moveData() {
    setMoving(true);
    try {
      await api.dataDirMove(movePath);
    } catch (e) {
      toastError(e);
      setMoving(false);
      setMoveOpen(false);
    }
  }

  return (
    <main className="scroll flex-1 min-w-0 px-10 pt-6 pb-12">
      <div className="enter max-w-[760px] flex flex-col">
        <h1 className="text-xl font-semibold tracking-[-0.01em] mb-2">Settings</h1>

        <Heading id="general">General</Heading>
        <Row title="Theme" hint="System follows Windows (dark or light).">
          <Segmented
            label="Theme"
            value={settings.theme}
            options={[{ value: "dark", label: "Dark" }, { value: "light", label: "Light" }, { value: "system", label: "System" }]}
            onChange={(theme) => set.mutate({ theme })}
          />
        </Row>
        <Row title="Accent colour" hint="Buttons, switches and highlights. Pick a preset or any colour.">
          <AccentPicker value={settings.accent} onChange={(accent) => set.mutate({ accent })} />
        </Row>
        <BackgroundRow dim={settings.backgroundDim} onDim={(backgroundDim) => set.mutate({ backgroundDim })} />
        <Row title="Density" hint="Row height in the accounts list.">
          <Segmented
            label="Density"
            value={settings.density}
            options={[{ value: "detailed", label: "Detailed" }, { value: "compact", label: "Compact" }]}
            onChange={(density) => set.mutate({ density })}
          />
        </Row>
        <Row title="Close to tray" hint="The window hides; running clients and auto-rejoin keep working.">
          <Switch label="Close to tray" checked={settings.closeToTray} onChange={(closeToTray) => set.mutate({ closeToTray })} />
        </Row>

        <Heading id="launching">Launching</Heading>
        <Row
          title="Multi-instance"
          hint="RoALT holds Roblox's single-instance lock while it runs, so each account gets its own client. Turn it on before the first client starts."
        >
          <Switch label="Multi-instance" checked={multi?.enabled ?? settings.multiInstance} onChange={multiToggle.toggle} />
          {multiToggle.dialog}
        </Row>
        <Row
          title="Protect each client's sign-in"
          hint="While RoALT's clients run, lock Roblox's shared sign-in files so clients can't overwrite each other's session (error 773, wrong account in game). Meanwhile Roblox's own app can't save its sign-in."
        >
          <Switch
            label="Protect each client's sign-in"
            checked={settings.protectSharedLogin}
            onChange={(protectSharedLogin) => set.mutate({ protectSharedLogin }, { onSuccess: () => qc.invalidateQueries({ queryKey: qk.multi }) })}
          />
        </Row>
        <Row title="Launch delay" hint="Wait between two launches when several accounts launch at once. About a second either way is added.">
          <NumberField
            unit="seconds between launches"
            short="s"
            min={2}
            max={60}
            step={0.5}
            value={settings.launchDelayMs / 1000}
            onCommit={(s) => set.mutate({ launchDelayMs: Math.round(s * 1000) })}
          />
        </Row>
        <Row
          title="Minimize on launch"
          hint="Each client is minimized once, when its window first appears, without taking the focus. Anti-AFK in focus mode skips minimized clients; restore one from the Dashboard."
        >
          <Switch label="Minimize on launch" checked={settings.minimizeOnLaunch} onChange={(minimizeOnLaunch) => set.mutate({ minimizeOnLaunch })} />
        </Row>
        <Row title="Auto-rejoin" hint="Relaunch a client Roblox disconnects (kicked, server shut down, connection lost) into the same target.">
          <Switch label="Auto-rejoin" checked={settings.autoRejoin} onChange={(autoRejoin) => set.mutate({ autoRejoin })} />
        </Row>
        <Row
          title="Auto-restart after a crash"
          hint="Relaunch a client that closes without RoALT closing it, or whose window stops responding for 30 s. Closing Roblox's window yourself looks the same, so close clients from RoALT."
        >
          <Switch
            label="Auto-restart after a crash"
            checked={settings.autoRestartOnCrash}
            onChange={(autoRestartOnCrash) => set.mutate({ autoRestartOnCrash })}
          />
        </Row>
        <Row
          title="Anti-AFK"
          hint="So Roblox doesn't disconnect idle clients after 20 minutes, each client in a game gets a Space press. A client can opt out under its limits in Launch."
        >
          <Switch label="Anti-AFK" checked={settings.antiAfk.enabled} onChange={(enabled) => set.mutate({ antiAfk: { ...settings.antiAfk, enabled } })} />
        </Row>
        {settings.antiAfk.enabled && (
          <Row
            title="How"
            hint={
              settings.antiAfk.mode === "focus"
                ? "Each client is brought to the front for a moment and the window you had in front comes back. Only after 5 s without keyboard or mouse input; minimized clients are skipped."
                : "The key is sent to each client's window without moving the focus, so you can keep using the PC, and minimized clients get it too. Roblox may not count it as activity in every game; auto-rejoin after a disconnect stays the fallback."
            }
          >
            <Segmented
              label="Anti-AFK mode"
              value={settings.antiAfk.mode}
              options={[
                { value: "focus", label: "Bring to front" },
                { value: "background", label: "In the background" },
              ]}
              onChange={(mode) => set.mutate({ antiAfk: { ...settings.antiAfk, mode } })}
            />
          </Row>
        )}
        {settings.antiAfk.enabled && (
          <Row title="Press a key every" hint="Roblox disconnects after 20 idle minutes, so 1 to 19.">
            <NumberField
              unit="minutes"
              short="min"
              min={1}
              max={19}
              value={settings.antiAfk.intervalMin}
              onCommit={(intervalMin) => set.mutate({ antiAfk: { ...settings.antiAfk, intervalMin } })}
            />
          </Row>
        )}
        <Row
          title="Give up after"
          hint="Rejoins and restarts of one account within this time. Each waits twice as long as the one before (at most a minute); past the limit RoALT leaves it and tells you."
        >
          <div className="flex flex-wrap gap-3">
            <NumberField unit="relaunches" short="tries" min={1} max={20} value={settings.breakerMax} onCommit={(breakerMax) => set.mutate({ breakerMax })} />
            <NumberField
              label="in"
              unit="minutes"
              short="min"
              min={1}
              max={240}
              value={settings.breakerWindowMin}
              onCommit={(breakerWindowMin) => set.mutate({ breakerWindowMin })}
            />
          </div>
        </Row>
        <Row title="Close grace" hint="How long a client RoALT closes gets to close by itself before it is ended.">
          <NumberField unit="seconds" short="s" min={1} max={60} value={settings.closeGraceS} onCommit={(closeGraceS) => set.mutate({ closeGraceS })} />
        </Row>
        <Row
          title="Launch arguments"
          hint="Extra words for every client, after its launch link (for a bootstrapper, say). No quotes or & | < > ^. An account can have its own in its details."
        >
          <TextField
            label="Launch arguments"
            className="w-64"
            placeholder="none"
            value={settings.launchArgs}
            onCommit={(launchArgs) => set.mutateAsync({ launchArgs })}
          />
        </Row>
        <Row
          title="FPS cap"
          hint="The frame rate Roblox is started with (its DFIntTaskSchedulerTargetFps flag, read once when a client starts), unless the account has its own. It applies to that launch only; a batch sets it before each launch. Not with a bootstrapper such as Bloxstrap, which manages Roblox's flags. 0 leaves Roblox alone."
        >
          <NumberField unit="FPS, 0 for off" short="FPS" min={0} max={360} value={settings.fpsCap} onCommit={(fpsCap) => set.mutate({ fpsCap })} />
        </Row>
        <ClientRow client={client} />
        <Row
          title="Update Roblox automatically"
          hint="When Roblox has a new version and no Roblox client runs (nor a launch or batch), install it. The installer is checked to be signed by Roblox Corporation first. Never over a bootstrapper such as Bloxstrap, which updates Roblox itself."
        >
          <Switch label="Update Roblox automatically" checked={settings.autoUpdate} onChange={(autoUpdate) => set.mutate({ autoUpdate })} />
        </Row>

        <Heading id="performance">Performance</Heading>
        <Row
          title="Restart on high memory"
          hint="Close and relaunch a client whose private memory stays over this for 30 s, into the same game, within the restart limit above. Not during a batch. 0 turns it off."
        >
          <NumberField unit="MB, 0 for off" short="MB" min={0} max={65536} value={settings.memRestartMb} onCommit={(memRestartMb) => set.mutate({ memRestartMb })} />
        </Row>
        <Row
          title="Throttle on high memory"
          hint="While a client's private memory is over this, hold it to the CPU limit beside it, which lowers its frame rate. Lifted once it is back under 90 %. Tightens a client's own limit, never loosens it. 0 turns it off."
        >
          <div className="flex flex-wrap items-center gap-3">
            <NumberField
              unit="MB, 0 for off"
              short="MB"
              min={0}
              max={65536}
              value={settings.memThrottle.mb}
              onCommit={(mb) => set.mutate({ memThrottle: { ...settings.memThrottle, mb } })}
            />
            <Segmented
              mono
              label="Throttle CPU limit"
              value={settings.memThrottle.cpuCap}
              options={[25, 10, 5, 2].map((c) => ({ value: c, label: `${c}%` }))}
              onChange={(cpuCap) => set.mutate({ memThrottle: { ...settings.memThrottle, cpuCap } })}
            />
          </div>
        </Row>
        <Row
          title="Trim memory every"
          hint="Ask Windows to page each client's memory out on a schedule; it comes back as used. A client's own setting in Launch wins. 0 turns it off."
        >
          <NumberField unit="minutes, 0 for off" short="min" min={0} max={1440} value={settings.trimEveryMin} onCommit={(trimEveryMin) => set.mutate({ trimEveryMin })} />
        </Row>
        <Row title="Spread clients over CPU cores" hint="Give each client its own share of the cores, round-robin. Cores picked for a client in Launch win.">
          <Switch label="Spread clients over CPU cores" checked={settings.autoSpread} onChange={(autoSpread) => set.mutate({ autoSpread })} />
        </Row>
        <Row
          title="Adaptive limits"
          hint="Clients whose window isn't in front get the limits below; the one you're playing runs free. A client with its own limits keeps them."
        >
          <Switch
            label="Adaptive limits"
            checked={settings.adaptive.enabled}
            onChange={(enabled) => set.mutate({ adaptive: { ...settings.adaptive, enabled } })}
          />
        </Row>
        {settings.adaptive.enabled && (
          <Row title="In the background" hint="CPU limit (a share of all cores), efficiency mode and how often to trim memory, for clients not in front.">
            <div className="flex flex-wrap items-center gap-3">
              <Segmented
                mono
                label="Background CPU limit"
                value={settings.adaptive.background.cpuCap ?? 0}
                options={[0, 50, 25, 10, 5].map((c) => ({ value: c, label: c ? `${c}%` : "Off" }))}
                onChange={(c) => set.mutate({ adaptive: { ...settings.adaptive, background: { ...settings.adaptive.background, cpuCap: c || null } } })}
              />
              <label className="flex items-center gap-2">
                <span className="text-xs text-dim">Efficiency</span>
                <Switch
                  label="Background efficiency mode"
                  checked={settings.adaptive.background.eco}
                  onChange={(eco) => set.mutate({ adaptive: { ...settings.adaptive, background: { ...settings.adaptive.background, eco } } })}
                />
              </label>
              <Segmented
                mono
                label="Background trim"
                value={settings.adaptive.background.trimEveryMin ?? -1}
                options={[-1, 0, 5, 15, 30, 60].map((m) => ({ value: m, label: m < 0 ? "Default" : m ? `${m}m` : "Off" }))}
                onChange={(m) =>
                  set.mutate({ adaptive: { ...settings.adaptive, background: { ...settings.adaptive.background, trimEveryMin: m < 0 ? null : m } } })
                }
              />
            </div>
          </Row>
        )}
        <BandwidthRow />

        <Heading id="automation">Automation</Heading>
        <Row
          title="Panic hotkey"
          hint="Works from any app, even a game in front: kills every Roblox client at once, like Kill all. Modifiers and one key, like Ctrl+Shift+F12. Empty turns it off."
        >
          <TextField
            label="Panic hotkey"
            className="w-48"
            placeholder="off"
            value={settings.panicHotkey}
            onCommit={(panicHotkey) => set.mutateAsync({ panicHotkey })}
          />
        </Row>
        <ShutdownRow />
        <Row
          title="Clean Roblox logs"
          hint="Delete Roblox's client logs over a day old or over 50 MB, at start and every hour. A running client's log is kept."
        >
          <div className="flex items-center gap-3">
            <CleanLogsButton />
            <Switch label="Clean Roblox logs" checked={settings.cleanLogs} onChange={(cleanLogs) => set.mutate({ cleanLogs })} />
          </div>
        </Row>

        <Heading id="security">Security</Heading>
        <Row
          title="Cookie encryption"
          hint={
            master
              ? "AES-256-GCM with a key from your master password (Argon2id). Asked at start and after auto-lock."
              : "Windows DPAPI: unlocks with your Windows sign-in, no prompt."
          }
        >
          <Segmented
            label="Cookie encryption"
            value={vault.mode}
            options={[{ value: "dpapi", label: "Windows account" }, { value: "master", label: "Master password" }]}
            onChange={(m) => setDlg(m === "master" ? "set" : "remove")}
          />
        </Row>
        {master && (
          <>
            <Row title="Master password" hint="Changing it only re-wraps the key; cookies are not re-encrypted.">
              <button className="btn" onClick={() => setDlg("change")}>Change…</button>
            </Row>
            <Row title="Auto-lock after idle" hint="Locks the vault and wipes the key from memory.">
              <Segmented
                label="Auto-lock"
                mono
                value={settings.autoLockMinutes}
                options={[0, 5, 15, 30, 60].map((m) => ({ value: m, label: m ? `${m} min` : "Off" }))}
                onChange={(autoLockMinutes) => set.mutate({ autoLockMinutes })}
              />
            </Row>
          </>
        )}
        <Row
          title="Clear clipboard after copying a cookie"
          hint={`Copying or revealing a cookie asks for ${master ? "your master password" : "Windows Hello"} first.`}
        >
          <NumberField
            unit="seconds"
            short="s"
            min={5}
            max={300}
            value={settings.clipboardClearSeconds}
            onCommit={(clipboardClearSeconds) => set.mutate({ clipboardClearSeconds })}
          />
        </Row>
        <RemoteRows settings={settings} />

        <Heading id="data">Data</Heading>
        <Row title="Data folder" hint={dataDir ? <PathHint path={dataDir.path} /> : ""}>
          <div className="flex gap-2">
            <button className="btn" onClick={() => api.dataDirOpen().catch(toastError)}>Open</button>
            <button className="btn" title="RoALT's own logs, for a bug report" onClick={() => api.logsOpen().catch(toastError)}>Open logs</button>
            {dataDir && !dataDir.overridden && <button className="btn" onClick={pickDataDir}>Move…</button>}
          </div>
        </Row>
        <Row
          title="Keep account info for"
          hint="How long the info panel reuses what Roblox said before asking again. Presence refreshes every minute while Accounts is open."
        >
          <NumberField
            unit="minutes"
            short="min"
            min={1}
            max={1440}
            value={settings.cacheMinutes}
            onCommit={(cacheMinutes) => set.mutate({ cacheMinutes })}
          />
        </Row>
        <Row title="Backup file" hint="One password-protected .roalt file with every account, its cookie and its groups.">
          <div className="flex gap-2">
            <button className="btn" onClick={pickImport}>
              <Icon name="upload" size={14} />
              Import…
            </button>
            <button className="btn btn-primary" onClick={() => setDlg("export")}>
              <Icon name="download" size={14} />
              Export…
            </button>
          </div>
        </Row>

        <Heading id="shortcuts">Shortcuts</Heading>
        <table className="w-full border-collapse mt-2">
          <tbody>
            {SHORTCUTS.map(([what, key]) => (
              <tr key={key} className="h-10 border-b border-line">
                <td>{what}</td>
                <td className="text-right"><span className="kbd">{key}</span></td>
              </tr>
            ))}
          </tbody>
        </table>

        <Heading id="about">About</Heading>
        <SelfUpdateRow on={settings.selfUpdate} onChange={(selfUpdate) => set.mutate({ selfUpdate })} />
        <Row title={<>RoALT <span className="mono">{import.meta.env.APP_VERSION}</span></>} hint="RoALT talks to roblox.com, to GitHub to look for its own updates (unless turned off above) and to Discord once you set a webhook, and changes nothing on your PC unless you apply a bandwidth limit. New versions are on GitHub Releases." last>
          <button className="btn" onClick={() => openUrl(RELEASES).catch(toastError)}>
            Open releases page
            <Icon name="external" size={12} />
          </button>
        </Row>
      </div>

      <PasswordDialog
        open={dlg === "set"}
        onClose={() => setDlg(null)}
        title="Set a master password"
        text="RoALT will ask for it at start and after auto-lock. There is no way to recover it; keep a backup."
        submitLabel="Set password"
        next
        nextLabel="Master password"
        onSubmit={({ next }) => setMode("master", undefined, next, "Master password set.")}
      />
      <PasswordDialog
        open={dlg === "change"}
        onClose={() => setDlg(null)}
        title="Change master password"
        text="Enter the current password, then the new one."
        submitLabel="Change password"
        current
        currentLabel="Current password"
        next
        onSubmit={({ current, next }) => setMode("master", current, next, "Master password changed.")}
      />
      <PasswordDialog
        open={dlg === "remove"}
        onClose={() => setDlg(null)}
        title="Remove master password"
        text="Cookies will be protected by your Windows account instead and open without a prompt."
        submitLabel="Remove password"
        current
        onSubmit={({ current }) => setMode("dpapi", current, undefined, "Master password removed. The vault now opens with Windows.")}
      />
      <PasswordDialog
        open={dlg === "export"}
        onClose={() => setDlg(null)}
        title="Export backup"
        text="The file holds every cookie, encrypted with this password. You need it to import the file."
        submitLabel="Choose file…"
        current={master}
        currentLabel="Master password"
        next
        nextLabel="Backup password"
        onSubmit={async ({ current, next }) => {
          const date = new Date().toISOString().slice(0, 10);
          const path = await saveFile({ defaultPath: `RoALT-backup-${date}.roalt`, filters: BACKUP_FILTER });
          if (!path) throw cancelled();
          const n = await api.backupExport(path, next, master ? current : undefined);
          toast(`Exported ${plural(n, "account")}.`);
        }}
      />
      <ImportDialog open={importOpen} path={importPath} onClose={() => setImportOpen(false)} />
      <ConfirmDialog
        open={moveOpen}
        onClose={() => !moving && setMoveOpen(false)}
        onConfirm={() => void moveData()}
        busy={moving}
        title="Move data folder?"
        text={
          <>
            RoALT copies its database to <span className="mono text-text break-all">{movePath}</span> and restarts to use it. The
            old database is deleted after the restart. Logs and the app's browser data stay in the old folder, so the theme and
            other view preferences reset.
          </>
        }
        confirmLabel={moving ? "Moving…" : "Move and restart"}
        safe
      />
    </main>
  );
}

function ImportDialog({ open, path, onClose }: { open: boolean; path: string; onClose: () => void }) {
  const [pw, setPw] = useState("");
  const [policy, setPolicy] = useState<ConflictPolicy>("skip");
  const [report, setReport] = useState<ImportReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) {
      setPw("");
      return;
    }
    setReport(null);
    setError(null);
  }, [open]);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!pw || busy) return;
    setBusy(true);
    setError(null);
    try {
      setReport(await api.backupImport(path, pw, policy));
      setPw("");
    } catch (err) {
      const e2 = asError(err);
      setError(e2.kind === "wrongPassword" || e2.kind === "crypto" ? "Wrong password, or the file is damaged." : friendly(e2));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onClose={onClose} title="Import backup">
      {report ? (
        <div className="enter flex flex-col">
          <div className="grid grid-cols-3 border-b border-line">
            {(["added", "updated", "skipped"] as const).map((k) => (
              <div key={k} className="px-5 py-4 border-r border-line last:border-r-0">
                <div className="label">{k}</div>
                <div className="mono text-[22px] font-medium mt-0.5">{report[k]}</div>
              </div>
            ))}
          </div>
          <div className="flex justify-end p-5">
            <button className="btn btn-primary" onClick={onClose} data-autofocus>Done</button>
          </div>
        </div>
      ) : (
        <form onSubmit={submit} className="flex flex-col gap-4 p-5">
          <div className="mono text-xs text-dim break-all border border-line px-2.5 py-2 selectable">{path.split(/[\\/]/).pop()}</div>
          <label className="flex flex-col gap-1.5">
            <span className="label">Backup password</span>
            <input
              type="password"
              className="field"
              value={pw}
              onChange={(e) => setPw(e.target.value)}
              autoComplete="off"
              aria-invalid={!!error}
            />
          </label>
          <div className="flex flex-col gap-1.5">
            <span className="label">Accounts already in RoALT</span>
            <Segmented
              label="Accounts already in RoALT"
              value={policy}
              options={[{ value: "skip", label: "Keep mine" }, { value: "replace", label: "Replace with backup" }]}
              onChange={setPolicy}
            />
          </div>
          {error && <div role="alert" className="alert">{error}</div>}
          <div className="flex justify-end gap-2 pt-1">
            <button type="button" className="btn" onClick={onClose}>Cancel</button>
            <button type="submit" className="btn btn-primary" disabled={busy || !pw}>{busy ? "Importing…" : "Import"}</button>
          </div>
        </form>
      )}
    </Dialog>
  );
}

/** The Roblox bandwidth limit: typed, then applied with admin rights only on the button. */
function BandwidthRow() {
  const qc = useQueryClient();
  const status = useQuery({ queryKey: qk.bandwidth, queryFn: api.bandwidthStatus, staleTime: 0 }).data;
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    if (status) setText(status.kbps ? String(status.kbps) : "");
  }, [status]);
  const apply = (kbps: number) => {
    setBusy(true);
    api
      .bandwidthApply(kbps)
      .then((st: BandwidthStatus) => {
        qc.setQueryData(qk.bandwidth, st);
        qc.invalidateQueries({ queryKey: qk.settings });
        toast(kbps ? `Roblox's upload is limited to ${kbps} kbit/s.` : "Roblox's bandwidth limit is off.");
      }, toastError)
      .finally(() => setBusy(false));
  };
  const n = Number(text || "0");
  const valid = Number.isInteger(n) && (n === 0 || (n >= 64 && n <= 10_000_000));
  const state = !status ? "" : !status.kbps ? "Off." : status.active ? `In force: ${status.kbps} kbit/s.` : `Set to ${status.kbps} kbit/s, but Windows has restarted since: apply it again.`;
  return (
    <Row
      title="Bandwidth limit"
      hint={
        <>
          Caps outbound (upload) traffic only, for all Roblox clients together, as a Windows QoS policy. Applying needs admin
          rights (Windows asks each time) and lasts until Windows restarts. {state}
        </>
      }
    >
      <div className="flex flex-wrap items-center gap-2">
        <label className="flex items-center gap-2">
          <span className="sr-only">kbit/s, empty or 0 for none</span>
          <input
            className="field mono w-24"
            type="number"
            min={0}
            max={10_000_000}
            placeholder="none"
            value={text}
            aria-invalid={!valid}
            onChange={(e) => setText(e.target.value)}
          />
          <span className="text-dim">kbit/s</span>
        </label>
        <button
          className="btn"
          disabled={busy || !valid}
          title={valid ? "Windows asks for admin rights" : "Use 64 to 10 000 000 kbit/s, or 0 for none"}
          onClick={() => apply(n)}
        >
          {busy ? "Waiting for Windows…" : "Apply (admin)"}
        </button>
        {status?.kbps !== 0 && status && (
          <button className="btn" disabled={busy} onClick={() => apply(0)}>Remove (admin)</button>
        )}
      </div>
    </Row>
  );
}

const SHUTDOWN_PRESETS = [30, 60, 120, 240, 480];

/** "1 h 05 min" style, for a preset or what is left. */
function duration(secs: number) {
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = Math.floor(secs % 60);
  return h ? `${h} h ${String(m).padStart(2, "0")} min` : m ? `${m} min ${String(s).padStart(2, "0")} s` : `${s} s`;
}

/** The next time the clock shows `hhmm` ("21:30"), as Unix ms. */
function nextAt(hhmm: string) {
  const [h, m] = hhmm.split(":").map(Number);
  const at = new Date();
  at.setHours(h, m, 0, 0);
  if (at.getTime() <= Date.now()) at.setDate(at.getDate() + 1);
  return at.getTime();
}

/** Auto-shutdown: Windows' own timer (shutdown /s), with a countdown and Cancel. */
function ShutdownRow() {
  const qc = useQueryClient();
  const at = useQuery({ queryKey: qk.shutdown, queryFn: api.shutdownStatus, staleTime: 0 }).data ?? null;
  const [time, setTime] = useState("");
  const [closeClients, setCloseClients] = useState(true);
  const [busy, setBusy] = useState(false);
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (at === null) return;
    const t = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(t);
  }, [at]);
  const run = (p: Promise<number | null>, done: (at: number | null) => string) => {
    setBusy(true);
    p.then((a) => {
      qc.setQueryData(qk.shutdown, a);
      toast(done(a));
    }, toastError).finally(() => setBusy(false));
  };
  const schedule = (when: { atMs?: number; inSecs?: number }) =>
    run(api.shutdownSchedule(when, closeClients), (a) => `Windows shuts down at ${new Date(a ?? 0).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}.`);
  return (
    <Row
      title="Shut down the PC"
      hint={
        <>
          Windows' own shutdown timer. When it runs out, Windows closes every app without asking, so unsaved work elsewhere is lost.
          {closeClients ? " RoALT's clients are closed 30 s before (if RoALT hasn't restarted since)." : ""} Cancel it here, even after a restart of RoALT.
        </>
      }
    >
      {at !== null ? (
        <div className="flex flex-wrap items-center gap-3">
          <span className="mono" role="timer">
            in {duration(Math.max(0, (at - now) / 1000))} · {new Date(at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}
          </span>
          <button className="btn" disabled={busy} onClick={() => run(api.shutdownCancel(), () => "The shutdown is off.")}>Cancel</button>
        </div>
      ) : (
        <div className="flex flex-col items-end gap-2">
          <div className="flex flex-wrap items-center gap-1.5">
            {SHUTDOWN_PRESETS.map((m) => (
              <button key={m} className="btn btn-sm mono" disabled={busy} onClick={() => schedule({ inSecs: m * 60 })}>
                {m < 60 ? `${m}m` : `${m / 60}h`}
              </button>
            ))}
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <input className="field mono" type="time" aria-label="Shut down at" value={time} onChange={(e) => setTime(e.target.value)} />
            <button className="btn btn-sm" disabled={busy || !time} onClick={() => schedule({ atMs: nextAt(time) })}>At this time</button>
            <label className="flex items-center gap-2">
              <span className="text-xs text-dim">Close clients first</span>
              <Switch label="Close clients first" checked={closeClients} onChange={setCloseClients} />
            </label>
          </div>
        </div>
      )}
    </Row>
  );
}

/** The LAN web dashboard: on/off (confirmed), its address, port and PIN (hidden until Reveal). */
function RemoteRows({ settings }: { settings: Settings }) {
  const qc = useQueryClient();
  const set = useSetSettings();
  const status = useQuery({ queryKey: qk.remote, queryFn: api.remoteStatus, staleTime: 0 }).data;
  const [pin, setPin] = useState<string | null>(null);
  const [confirmOn, setConfirmOn] = useState(false);
  const remote = settings.remote;
  const save = (r: Settings["remote"]) => set.mutate({ remote: r }, { onSettled: () => qc.invalidateQueries({ queryKey: qk.remote }) });
  const reveal = () => (pin ? setPin(null) : api.remotePin().then(setPin, toastError));
  const regenerate = () =>
    api.remotePinRegenerate().then((p) => {
      setPin(p);
      toast("New PIN. Every signed-in device has to enter it again.");
    }, toastError);
  return (
    <>
      <Row
        title="Web dashboard"
        hint={
          <>
            See, launch and close clients from a phone or another PC on your network, in a browser. Anyone on the network with the PIN
            can launch and close your clients. Runs only while the vault is unlocked.
          </>
        }
      >
        <Switch label="Web dashboard" checked={remote.enabled} onChange={(enabled) => (enabled ? setConfirmOn(true) : save({ ...remote, enabled }))} />
      </Row>
      {remote.enabled && (
        <>
          <Row title="Address" hint={status?.running ? "Open it on a device on the same network." : "Not running. Unlock the vault, or pick another port."}>
            <div className="flex flex-col items-end gap-1">
              {status?.urls.length ? (
                status.urls.map((u) => <span key={u} className="mono selectable">{u}</span>)
              ) : (
                <span className="text-dim">No network found.</span>
              )}
            </div>
          </Row>
          <Row title="Port" hint="1024 to 65535. Changing it restarts the dashboard.">
            <NumberField unit="port number" short="" min={1024} max={65535} value={remote.port} onCommit={(port) => save({ ...remote, port })} />
          </Row>
          <Row title="PIN" hint="Asked once on each device. Regenerating signs every device out.">
            <div className="flex items-center gap-2">
              <span className="mono selectable" aria-live="polite">{pin ?? "••••••••"}</span>
              <button className="btn btn-sm" onClick={reveal}>{pin ? "Hide" : "Reveal"}</button>
              <button className="btn btn-sm" onClick={regenerate}>Regenerate</button>
            </div>
          </Row>
        </>
      )}
      <ConfirmDialog
        open={confirmOn}
        onClose={() => setConfirmOn(false)}
        onConfirm={() => {
          setConfirmOn(false);
          save({ ...remote, enabled: true });
        }}
        title="Turn on the web dashboard?"
        text={
          <>
            Anyone on your network who has the PIN can see your clients, launch your accounts and close or kill clients. It never shows
            cookies or other secrets. Windows Firewall asks to allow RoALT the first time: allow private networks only, never public ones.
          </>
        }
        confirmLabel="Turn on"
      />
    </>
  );
}

/** Deletes old Roblox logs now and says how much it freed. */
function CleanLogsButton() {
  const [busy, setBusy] = useState(false);
  const clean = () => {
    setBusy(true);
    api
      .logsClean()
      .then(
        (r) => toast(r.files ? `Deleted ${plural(r.files, "Roblox log")}, ${(r.bytes / 1048576).toFixed(1)} MB.` : "No Roblox log to delete."),
        toastError,
      )
      .finally(() => setBusy(false));
  };
  return (
    <button className="btn btn-sm" disabled={busy} onClick={clean}>
      {busy ? "Cleaning…" : "Clean now"}
    </button>
  );
}

export function Heading({ id, children }: { id: string; children: ReactNode }) {
  return <h2 id={id} className="label mt-8 first-of-type:mt-6 scroll-mt-6">{children}</h2>;
}

function PathHint({ path }: { path: string }) {
  return <span className="mono block truncate selectable" title={path}>{path}</span>;
}

const ROBLOX_DOWNLOAD = "https://www.roblox.com/download";

function installStep(p: InstallProgress | null) {
  switch (p?.stage) {
    case "downloading":
      return `Downloading… ${p.percent}%`;
    case "verifying":
      return "Checking the signature…";
    case "installing":
      return "Roblox's installer is running…";
    default:
      return null;
  }
}

/** The self-updater's switch and Check now, with what the check found. */
function SelfUpdateRow({ on, onChange }: { on: boolean; onChange: (on: boolean) => void }) {
  const qc = useQueryClient();
  const [result, setResult] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function check() {
    setBusy(true);
    setResult(null);
    try {
      const u = await api.selfUpdateCheck();
      unhideUpdate();
      qc.setQueryData(qk.appUpdate, u);
      setResult(u.available ? `RoALT ${u.available} is available.` : "RoALT is up to date.");
    } catch (e) {
      setResult(friendly(asError(e)));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Row
      title="Check for RoALT updates"
      hint="Asks GitHub Releases at start and every 6 hours. A new version is offered, never installed without your click, and its installer runs only if it carries RoALT's release signature."
    >
      <span className="flex flex-wrap items-center gap-3">
        {result && <span className="text-xs text-dim" role="status">{result}</span>}
        <button className="btn btn-sm" disabled={busy} onClick={() => void check()}>
          {busy ? "Checking…" : "Check now"}
        </button>
        <Switch label="Check for RoALT updates" checked={on} onChange={onChange} />
      </span>
    </Row>
  );
}

/** The client RoALT starts, its version, and Install / Update Roblox (Roblox's own client only). */
function ClientRow({ client }: { client: RobloxClient | undefined }) {
  const qc = useQueryClient();
  const [progress, setProgress] = useState<InstallProgress | null>(null);
  const [failed, setFailed] = useState(false);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    const un = events.onRobloxInstall(setProgress);
    return () => void un.then((f) => f());
  }, []);
  const step = installStep(progress);
  const offer = client && !client.bootstrapper;

  async function install() {
    if (busy) return;
    setBusy(true);
    setFailed(false);
    try {
      const c = await api.robloxInstall();
      qc.setQueryData(qk.client, c);
      toast(c.version ? `Roblox ${c.version} is installed.` : "Roblox is installed.");
    } catch (e) {
      setFailed(true);
      toastError(e);
    } finally {
      setBusy(false);
    }
  }

  const version = client?.path && !client.bootstrapper && (
    <span className="mono block truncate">
      {client.version ?? "version unknown"}
      {client.outdated && client.latest ? ` · ${client.latest} is out` : client.latest && client.version === client.latest ? " · latest" : ""}
    </span>
  );
  return (
    <Row
      title="Roblox client"
      hint={
        client?.path ? (
          <>
            <PathHint path={client.path} />
            {version}
          </>
        ) : client ? (
          "Roblox isn't installed. Install it here or from roblox.com."
        ) : (
          ""
        )
      }
    >
      <span className="flex flex-wrap items-center gap-3">
        {client && <ClientStatus client={client} />}
        {step ? (
          <span className="text-xs text-dim" role="status">{step}</span>
        ) : (
          offer && (
            <button className="btn btn-sm" disabled={busy} onClick={() => void install()} title="Downloads Roblox's installer from setup.rbxcdn.com and runs it once it is verified as signed by Roblox Corporation">
              {!client.path ? "Install Roblox" : client.outdated ? "Update Roblox" : "Reinstall Roblox"}
            </button>
          )
        )}
        {failed && !step && (
          <button className="btn btn-sm" onClick={() => openUrl(ROBLOX_DOWNLOAD).catch(toastError)}>
            <Icon name="external" size={11} />
            roblox.com/download
          </button>
        )}
      </span>
    </Row>
  );
}

function ClientStatus({ client }: { client: RobloxClient }) {
  const [label, color] = !client.path
    ? ["not installed", "var(--orange)"]
    : client.bootstrapper
      ? ["via a bootstrapper", "var(--blue)"]
      : client.outdated
        ? ["outdated", "var(--orange)"]
        : ["detected", "var(--accent-text)"];
  return (
    <span className="mono inline-flex items-center gap-1.5 text-xs">
      <span aria-hidden="true" className="w-1.5 h-1.5 flex-none" style={{ background: color }} />
      {label}
    </span>
  );
}

function AccentPicker({ value, onChange }: { value: string; onChange: (v: string) => void }) {
  const custom = !!value && !ACCENTS.some(([v]) => v === value);
  return (
    <div role="group" aria-label="Accent colour" className="flex flex-wrap items-center gap-1.5">
      {ACCENTS.map(([v, name]) => (
        <button
          key={name}
          type="button"
          title={name}
          aria-label={name}
          aria-pressed={v === value}
          className={`w-6 h-6 border ${v === value ? "border-text outline outline-1 outline-text outline-offset-1" : "border-line-strong"}`}
          style={{ background: v || "#c6f432" }}
          onClick={() => v !== value && onChange(v)}
        />
      ))}
      <label title="Any colour" className={`relative w-6 h-6 border cursor-pointer ${custom ? "border-text outline outline-1 outline-text outline-offset-1" : "border-line-strong"}`}
        style={{ background: custom ? value : "conic-gradient(red, yellow, lime, cyan, blue, magenta, red)" }}>
        <span className="sr-only">Custom colour</span>
        <input type="color" className="absolute inset-0 opacity-0 cursor-pointer" value={value || "#c6f432"} onChange={(e) => onChange(e.target.value)} />
      </label>
    </div>
  );
}

/** Pick, replace or remove the window's background image, and how much the theme covers it. */
function BackgroundRow({ dim, onDim }: { dim: number; onDim: (v: number) => void }) {
  const qc = useQueryClient();
  const has = !!useQuery({ queryKey: qk.background, queryFn: api.backgroundGet, staleTime: Infinity }).data?.byteLength;
  const done = () => qc.invalidateQueries({ queryKey: qk.background });
  const pick = async () => {
    const path = await openFile({ multiple: false, directory: false, filters: PICKER_FILTER, title: "Choose a background image" }).catch(toastError);
    if (path) api.backgroundSet(path).then(done, toastError);
  };
  return (
    <Row title="Background image" hint="Your own picture behind the app (PNG, JPEG, WebP or GIF, up to 8 MB). It is kept in RoALT's data.">
      <div className="flex flex-wrap items-center gap-2">
        {has && (
          <Segmented
            label="How much the theme covers the picture"
            value={dim}
            options={[50, 70, 80, 90].map((v) => ({ value: v, label: `${v}%`, title: `The theme covers ${v}% of the picture` }))}
            onChange={onDim}
            mono
          />
        )}
        <button className="btn" onClick={pick}>{has ? "Change…" : "Choose…"}</button>
        {has && <button className="btn" onClick={() => api.backgroundClear().then(done, toastError)}>Remove</button>}
      </div>
    </Row>
  );
}

export function Row({ title, hint, children, last }: { title: ReactNode; hint: ReactNode; children: ReactNode; last?: boolean }) {
  return (
    <div className={`flex flex-wrap items-center gap-x-6 gap-y-2 py-4 ${last ? "" : "border-b border-line"}`}>
      <div className="flex-[1_1_280px] min-w-0 flex flex-col gap-0.5">
        <span>{title}</span>
        <span className="text-xs text-dim">{hint}</span>
      </div>
      {children}
    </div>
  );
}
