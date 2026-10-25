import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { open as openFile, save as saveFile } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, asError, type ConflictPolicy, type ImportReport, type Settings, type VaultStatus } from "../../api";
import { qk, useMultiStatus, useSetSettings } from "../../data";
import { Segmented, Switch } from "../../ui/controls";
import { Dialog } from "../../ui/Dialog";
import { Icon } from "../../ui/Icon";
import { friendly, toast, toastError } from "../../ui/Toast";
import { plural } from "../accounts/shared";
import { useMultiToggle } from "../launch/Running";
import { PasswordDialog } from "../vault/PasswordDialog";

export const SETTINGS_SECTIONS = [
  ["general", "General"],
  ["launching", "Launching"],
  ["security", "Security"],
  ["data", "Data"],
  ["shortcuts", "Shortcuts"],
  ["about", "About"],
] as const;

const SHORTCUTS = [
  ["Add account", "Ctrl N"],
  ["Filter accounts", "Ctrl F"],
  ["Select all shown", "Ctrl A"],
  ["Validate selected, or all", "Ctrl R"],
  ["Remove selected", "Delete"],
  ["Launch (Launch view)", "Ctrl Enter"],
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

  return (
    <main className="scroll flex-1 min-w-0 px-10 pt-6 pb-12">
      <div className="enter max-w-[760px] flex flex-col">
        <h1 className="text-xl font-semibold tracking-[-0.01em] mb-2">Settings</h1>

        <Heading id="general">General</Heading>
        <Row title="Theme" hint="System follows Windows.">
          <Segmented
            label="Theme"
            value={settings.theme}
            options={[{ value: "dark", label: "Dark" }, { value: "light", label: "Light" }, { value: "system", label: "System" }]}
            onChange={(theme) => set.mutate({ theme })}
          />
        </Row>
        <Row title="Density" hint="Row height in the accounts list.">
          <Segmented
            label="Density"
            value={settings.density}
            options={[{ value: "detailed", label: "Detailed" }, { value: "compact", label: "Compact" }]}
            onChange={(density) => set.mutate({ density })}
          />
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

        <Heading id="data">Data</Heading>
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
        <Row title={<>RoALT <span className="mono">0.1.0</span></>} hint="RoALT only talks to roblox.com. New versions are on GitHub Releases." last>
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

/** Whole-number input that saves on blur or Enter and snaps back when out of range. */
function NumberField({ unit, short, min, max, value, onCommit }: {
  unit: string;
  short: string;
  min: number;
  max: number;
  value: number;
  onCommit: (n: number) => void;
}) {
  const [text, setText] = useState(String(value));
  useEffect(() => setText(String(value)), [value]);
  const commit = () => {
    const n = Number(text);
    if (n === value) return;
    if (Number.isInteger(n) && n >= min && n <= max) onCommit(n);
    else {
      toast(`Use a whole number of ${unit} from ${min} to ${max}.`, "error");
      setText(String(value));
    }
  };
  return (
    <label className="flex items-center gap-2">
      <span className="sr-only">{unit}</span>
      <input
        className="field mono w-20"
        type="number"
        min={min}
        max={max}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
      />
      <span className="text-dim">{short}</span>
    </label>
  );
}

function Heading({ id, children }: { id: string; children: ReactNode }) {
  return <h2 id={id} className="label mt-8 first-of-type:mt-6 scroll-mt-6">{children}</h2>;
}

function Row({ title, hint, children, last }: { title: ReactNode; hint: string; children: ReactNode; last?: boolean }) {
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
