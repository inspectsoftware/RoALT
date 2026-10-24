import { useEffect, useState, type FormEvent } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api, asError, type AccountSummary } from "../../api";
import { qk } from "../../data";
import { Dialog } from "../../ui/Dialog";
import { Icon } from "../../ui/Icon";
import { friendly, toast } from "../../ui/Toast";

type Tab = "login" | "cookie";

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
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (open) return;
    setCookie("");
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

  const tabBtn = (t: Tab, label: string, icon: "signin" | "key") => (
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
