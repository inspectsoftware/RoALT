import { useEffect, useState, type FormEvent } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { open as openFile } from "@tauri-apps/plugin-dialog";
import { api, asError, type AccountSummary, type ImportReport } from "../../api";
import { qk } from "../../data";
import { Dialog } from "../../ui/Dialog";
import { Icon } from "../../ui/Icon";
import { friendly, toast } from "../../ui/Toast";
import { fmtList, t, tx } from "../../i18n";

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
    toast(t("accounts.add.added", { name: a.username }));
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
    const parts = [
      t("accounts.add.importAdded", { count: r.added }),
      r.updated && t("accounts.add.importRefreshed", { count: r.updated }),
      r.skipped && t("accounts.add.importRepeated", { count: r.skipped }),
      r.failed && t("accounts.add.importFailed", { count: r.failed }),
    ].filter((p): p is string => !!p);
    toast(t("accounts.add.importResult", { list: fmtList(parts) }));
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
    const path = await openFile({ multiple: false, directory: false, filters: RAM_FILTER, title: t("accounts.add.ramPickerTitle") }).catch((e) => {
      setError(friendly(asError(e)));
      return null;
    });
    if (path) runImport(() => api.accountsImportRam(path));
  }

  const tabBtn = (id: Tab, label: string, icon: "signin" | "key" | "upload") => (
    <button
      type="button"
      role="tab"
      id={`add-tab-${id}`}
      aria-selected={tab === id}
      aria-controls={`add-panel-${id}`}
      onClick={() => {
        setTab(id);
        setError(null);
      }}
      className={`flex-1 min-w-0 flex items-center justify-center gap-2 min-h-10 py-1 text-center border-b-2 ${
        tab === id ? "border-[var(--on)] text-text" : "border-transparent text-dim hover:text-text"
      }`}
    >
      <Icon name={icon} size={14} />
      {label}
    </button>
  );

  return (
    <Dialog open={open} onClose={onClose} title={t("accounts.add.title")} width={460}>
      <div role="tablist" aria-label={t("accounts.add.how")} className="flex px-5 border-b border-line">
        {tabBtn("login", t("accounts.add.tabLogin"), "signin")}
        {tabBtn("cookie", t("accounts.add.tabCookie"), "key")}
        {tabBtn("many", t("accounts.add.tabMany"), "upload")}
      </div>

      {tab === "login" ? (
        <div key="login" role="tabpanel" id="add-panel-login" aria-labelledby="add-tab-login" className="enter flex flex-col gap-4 p-5">
          <p className="text-dim">
            {t("accounts.add.loginIntro")}
          </p>
          <p className="text-dim text-xs">
            {t("accounts.add.loginPrivate")}
          </p>
          {waiting ? (
            <div className="enter flex flex-col gap-2.5 py-1" role="status">
              <div className="live-bar" />
              <span>{t("accounts.add.waiting")}</span>
            </div>
          ) : (
            <button type="button" className="btn btn-primary self-start" onClick={signIn} data-autofocus>
              <Icon name="external" size={14} />
              {t("accounts.add.openSignIn")}
            </button>
          )}
          {error && <div role="alert" className="alert">{error}</div>}
        </div>
      ) : tab === "many" ? (
        <form key="many" role="tabpanel" id="add-panel-many" aria-labelledby="add-tab-many" onSubmit={pasteMany} className="enter flex flex-col gap-3 p-5">
          <label className="flex flex-col gap-1.5">
            <span className="label">{t("accounts.add.manyLabel")}</span>
            <textarea
              className="field secret mono text-xs"
              rows={6}
              value={many}
              onChange={(e) => setMany(e.target.value)}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
              placeholder={t("accounts.add.manyPlaceholder")}
            />
          </label>
          <p className="text-xs text-dim">
            {tx("accounts.add.manyHelp", {}, { mono: (s) => <span className="mono">{s}</span> })}
          </p>
          {busy && <div className="live-bar" role="status" aria-label={t("accounts.add.importing")} />}
          {error && <div role="alert" className="alert">{error}</div>}
          <div className="flex flex-wrap justify-between gap-2 pt-1">
            <button type="button" className="btn" onClick={fromRam} disabled={busy} title={t("accounts.add.ramHint")}>
              <Icon name="upload" size={14} />
              {t("accounts.add.ramImport")}
            </button>
            <button type="submit" className="btn btn-primary" disabled={busy || !many.trim()}>
              {busy ? t("accounts.add.importingEllipsis") : t("accounts.add.import")}
            </button>
          </div>
        </form>
      ) : (
        <form key="cookie" role="tabpanel" id="add-panel-cookie" aria-labelledby="add-tab-cookie" onSubmit={paste} className="enter flex flex-col gap-3 p-5">
          <label className="flex flex-col gap-1.5">
            <span className="label">{t("accounts.add.cookieLabel")}</span>
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
              placeholder={t("accounts.add.cookiePlaceholder")}
            />
          </label>
          <p className="text-xs text-dim">
            {tx("accounts.add.cookieHelp", {}, { mono: (s) => <span className="mono">{s}</span> })}
          </p>
          {error && <div role="alert" className="alert">{error}</div>}
          <div className="flex justify-end gap-2 pt-1">
            <button type="button" className="btn" onClick={onClose}>{t("common.cancel")}</button>
            <button type="submit" className="btn btn-primary" disabled={busy || !cookie.trim()}>
              {busy ? t("accounts.status.checking") : t("accounts.add.title")}
            </button>
          </div>
        </form>
      )}
    </Dialog>
  );
}
