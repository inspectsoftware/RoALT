import { useEffect, useState, type FormEvent } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { api, asError, type AccountSummary, type ImportReport } from "../../api";
import { qk } from "../../data";
import { Dialog } from "../../ui/Dialog";
import { Icon } from "../../ui/Icon";
import { friendly, toast } from "../../ui/Toast";

type Tab = "login" | "cookie" | "many";

const RAM_FILTER = [{ name: "Roblox Account Manager", extensions: ["json"] }];

export function AddAccountDialog({ open, onClose, onAdded }: {
  open: boolean;
  onClose: () => void;
  onAdded: (a: AccountSummary) => void;
}) {
  const qc = useQueryClient();
  const [tab, setTab] = useState<Tab>("login");
  const [waiting, setWaiting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [cookie, setCookie] = useState("");
  const [many, setMany] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (open) return;
    setCookie("");
    setMany("");
    setError(null);
  }, [open]);

  const added = (a: AccountSummary) => {
    qc.invalidateQueries({ queryKey: qk.accounts });
    qc.invalidateQueries({ queryKey: qk.vault });
    toast(`Added @${a.username}`);
    onAdded(a);
    onClose();
  };

  async function signIn() {
    setError(null);
    setWaiting(true);
    try {
      added(await api.accountAddLogin());
    } catch (e) {
      const err = asError(e);
      if (err.kind !== "cancelled") setError(friendly(err));
    } finally {
      setWaiting(false);
    }
  }

  async function paste(e: FormEvent) {
    e.preventDefault();
    const value = cookie;
    setCookie(""); // the field never keeps a submitted cookie
    if (!value.trim() || busy) return;
    setError(null);
    setBusy(true);
    try {
      added(await api.accountAddCookie(value));
    } catch (e) {
      setError(friendly(asError(e)));
    } finally {
      setBusy(false);
    }
  }

  const imported = (r: ImportReport) => {
    qc.invalidateQueries({ queryKey: qk.accounts });
    qc.invalidateQueries({ queryKey: qk.vault });
    const parts = [`${r.added} added`, r.updated && `${r.updated} refreshed`, r.skipped && `${r.skipped} repeated`, r.failed && `${r.failed} failed`];
    toast(`Import: ${parts.filter(Boolean).join(", ")}.`);
    if (r.added + r.updated > 0) onClose();
  };

  /** Pasted lists and Roblox Account Manager files both go one cookie at a time. */
  async function runImport(run: () => Promise<ImportReport>) {
    setError(null);
    setBusy(true);
    try {
      imported(await run());
    } catch (e) {
      setError(friendly(asError(e)));
    } finally {
      setBusy(false);
    }
  }

  function pasteMany(e: FormEvent) {
    e.preventDefault();
    const value = many;
    setMany(""); // as with one cookie, the field never keeps what was submitted
    if (!value.trim() || busy) return;
    runImport(() => api.accountsImportCookies(value));
  }

  async function fromRam() {
    if (busy) return;
    const path = await openFile({ multiple: false, directory: false, filters: RAM_FILTER, title: "Choose Roblox Account Manager's AccountData.json" }).catch((e) => {
      setError(friendly(asError(e)));
      return null;
    });
    if (path) runImport(() => api.accountsImportRam(path));
  }

  const tabBtn = (t: Tab, label: string, icon: "signin" | "key" | "upload") => (
    <button
      type="button"
      role="tab"
      id={`add-tab-${t}`}
      aria-selected={tab === t}
      aria-controls={`add-panel-${t}`}
      onClick={() => {
        setTab(t);
        setError(null);
      }}
      className={`flex-1 flex items-center justify-center gap-2 h-10 border-b-2 ${
        tab === t ? "border-[var(--on)] text-text" : "border-transparent text-dim hover:text-text"
      }`}
    >
      <Icon name={icon} size={14} />
      {label}
    </button>
  );

  return (
    <Dialog open={open} onClose={onClose} title="Add account" width={460}>
      <div role="tablist" aria-label="How to add" className="flex px-5 border-b border-line">
        {tabBtn("login", "Sign in", "signin")}
        {tabBtn("cookie", "Paste cookie", "key")}
        {tabBtn("many", "Paste many", "upload")}
      </div>

      {tab === "login" ? (
        <div key="login" role="tabpanel" id="add-panel-login" aria-labelledby="add-tab-login" className="enter flex flex-col gap-4 p-5">
          <p className="text-dim">
            A Roblox sign-in window opens. Sign in as usual: passkeys, security keys and 2-step verification work there.
            RoALT keeps the session and closes the window by itself.
          </p>
          <p className="text-dim text-xs">
            The window uses a fresh private browser profile that is deleted afterwards. Close it to cancel.
          </p>
          {waiting ? (
            <div className="enter flex flex-col gap-2.5 py-1" role="status">
              <div className="live-bar" />
              <span>Waiting for you to finish signing in…</span>
            </div>
          ) : (
            <button type="button" className="btn btn-primary self-start" onClick={signIn} data-autofocus>
              <Icon name="external" size={14} />
              Open Roblox sign-in
            </button>
          )}
          {error && <div role="alert" className="alert">{error}</div>}
        </div>
      ) : tab === "many" ? (
        <form key="many" role="tabpanel" id="add-panel-many" aria-labelledby="add-tab-many" onSubmit={pasteMany} className="enter flex flex-col gap-3 p-5">
          <label className="flex flex-col gap-1.5">
            <span className="label">Cookies, one per line</span>
            <textarea
              className="field secret mono text-xs"
              rows={6}
              value={many}
              onChange={(e) => setMany(e.target.value)}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
              placeholder="_|WARNING:-DO-NOT-SHARE-THIS…  or  user:pass:_|WARNING:…"
            />
          </label>
          <p className="text-xs text-dim">
            Every .ROBLOSECURITY cookie in the text is checked with Roblox one by one, so a long list takes a while.
            Lines like <span className="mono">user:pass:cookie</span> work too; only the cookie is kept.
          </p>
          {busy && <div className="live-bar" role="status" aria-label="Importing" />}
          {error && <div role="alert" className="alert">{error}</div>}
          <div className="flex justify-between gap-2 pt-1">
            <button type="button" className="btn" onClick={fromRam} disabled={busy} title="Its AccountData.json; one locked with a RAM password can't be read">
              <Icon name="upload" size={14} />
              Import from Roblox Account Manager
            </button>
            <button type="submit" className="btn btn-primary" disabled={busy || !many.trim()}>
              {busy ? "Importing…" : "Import"}
            </button>
          </div>
        </form>
      ) : (
        <form key="cookie" role="tabpanel" id="add-panel-cookie" aria-labelledby="add-tab-cookie" onSubmit={paste} className="enter flex flex-col gap-3 p-5">
          <label className="flex flex-col gap-1.5">
            <span className="label">.ROBLOSECURITY cookie</span>
            <textarea
              className="field secret mono text-xs"
              rows={4}
              value={cookie}
              onChange={(e) => setCookie(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && !e.shiftKey && (e.preventDefault(), e.currentTarget.form?.requestSubmit())}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
              placeholder="Paste the value here"
            />
          </label>
          <p className="text-xs text-dim">
            The raw value or <span className="mono">.ROBLOSECURITY=…</span> both work. RoALT checks it with Roblox, then keeps it
            encrypted on this PC. Anyone with this cookie can sign in as you; never paste it anywhere else.
          </p>
          {error && <div role="alert" className="alert">{error}</div>}
          <div className="flex justify-end gap-2 pt-1">
            <button type="button" className="btn" onClick={onClose}>Cancel</button>
            <button type="submit" className="btn btn-primary" disabled={busy || !cookie.trim()}>
              {busy ? "Checking…" : "Add account"}
            </button>
          </div>
        </form>
      )}
    </Dialog>
  );
}
