import { useRef, useState, type FormEvent } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api, asError, type VaultStatus } from "../../api";
import { qk } from "../../data";
import { Icon } from "../../ui/Icon";
import { friendly } from "../../ui/Toast";
import { t } from "../../i18n";

export type LockReason = "start" | "manual" | "idle";

export function UnlockScreen({ status, reason }: { status: VaultStatus; reason: LockReason }) {
  const qc = useQueryClient();
  const input = useRef<HTMLInputElement>(null);
  const [pw, setPw] = useState("");
  const [peek, setPeek] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [shakes, setShakes] = useState(0);
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!pw || busy) return;
    setBusy(true);
    try {
      await api.vaultUnlock(pw);
      setPw("");
      qc.setQueryData<VaultStatus>(qk.vault, { ...status, locked: false });
      qc.invalidateQueries();
    } catch (err) {
      const e2 = asError(err);
      setError(e2.kind === "wrongPassword" ? t("ui.error.wrongPassword") : friendly(e2));
      setShakes((n) => n + 1);
      input.current?.select();
    } finally {
      setBusy(false);
    }
  }

  const lead = reason === "idle" ? t("vault.lockedIdle", { count: status.autoLockMinutes }) : reason === "manual" ? t("vault.lockedManual") : "";

  return (
    <main className="gridbg flex-1 grid place-items-center px-4 py-10 overflow-auto">
      <form
        onSubmit={submit}
        className={`enter ${shakes ? (shakes % 2 ? "shake" : "shake2") : ""} w-full max-w-[380px] flex flex-col gap-5 p-8 bg-panel border border-line-strong`}
      >
        <div className="flex flex-col gap-3.5">
          <svg width="36" height="36" viewBox="0 0 36 36" aria-hidden="true">
            <rect x="6" y="15" width="24" height="17" fill="none" stroke="currentColor" strokeWidth="2" />
            <path d="M11 15V5h14v10" fill="none" stroke="currentColor" strokeWidth="2" />
            <rect x="16" y="21" width="4" height="5" fill="var(--accent)" />
          </svg>
          <h1 className="text-[22px] font-semibold tracking-[-0.01em]">{t("vault.title")}</h1>
          <p className="text-dim">
            {lead && `${lead} `}
            {t("vault.prompt", { count: status.accounts })}
          </p>
        </div>

        <div className="flex flex-col gap-2">
          <label className="label" htmlFor="unlock-pw">{t("settings.masterPassword")}</label>
          <div className="flex border bg-bg" style={{ borderColor: error ? "var(--orange)" : "var(--line-strong)" }}>
            <input
              ref={input}
              id="unlock-pw"
              type={peek ? "text" : "password"}
              value={pw}
              onChange={(e) => {
                setPw(e.target.value);
                setError(null);
              }}
              autoComplete="current-password"
              autoFocus
              spellCheck={false}
              aria-invalid={!!error}
              aria-describedby={error ? "unlock-err" : undefined}
              className="flex-1 min-w-0 h-10 px-3 bg-transparent outline-none tracking-[0.08em]"
            />
            <button
              type="button"
              className="wc h-10 w-11"
              aria-label={peek ? t("vault.hidePassword") : t("vault.showPassword")}
              aria-pressed={peek}
              onClick={() => setPeek(!peek)}
            >
              <Icon name={peek ? "eyeOff" : "eye"} />
            </button>
          </div>
          {error && <div id="unlock-err" role="alert" className="alert">{error}</div>}
        </div>

        <button type="submit" className="btn btn-primary h-10" disabled={busy}>
          {busy ? t("vault.unlocking") : t("vault.unlock")}
        </button>

        <div className="mono flex flex-col gap-1 pt-4 border-t border-line text-[11px] text-dim">
          <span>{t("vault.cipher")}</span>
          <span>{t("vault.local")}</span>
        </div>
      </form>
    </main>
  );
}
