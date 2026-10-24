import { useEffect, useState, type FormEvent, type ReactNode } from "react";
import { asError } from "../../api";
import { Dialog } from "../../ui/Dialog";
import { friendly } from "../../ui/Toast";

export const MIN_PASSWORD = 8;

/**
 * Asks for the current password, a new one (with confirmation), or both.
 * onSubmit rejecting with kind "cancelled" keeps the dialog open quietly; any other
 * rejection shows inline. Passwords are dropped from state when the dialog closes.
 */
export function PasswordDialog({
  open, onClose, title, text, submitLabel, onSubmit,
  current = false, next = false, currentLabel = "Master password", nextLabel = "New password",
}: {
  open: boolean;
  onClose: () => void;
  title: string;
  text: ReactNode;
  submitLabel: string;
  onSubmit: (v: { current: string; next: string }) => Promise<unknown>;
  current?: boolean;
  next?: boolean;
  currentLabel?: string;
  nextLabel?: string;
}) {
  const [cur, setCur] = useState("");
  const [nw, setNw] = useState("");
  const [confirm, setConfirm] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [shakes, setShakes] = useState(0);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (open) return;
    setCur("");
    setNw("");
    setConfirm("");
    setError(null);
    setShakes(0);
  }, [open]);

  const fail = (msg: string) => {
    setError(msg);
    setShakes((n) => n + 1);
  };

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (busy) return;
    if (current && !cur) return fail(`Enter your ${currentLabel.toLowerCase()}.`);
    if (next && nw.length < MIN_PASSWORD) return fail(`Use at least ${MIN_PASSWORD} characters.`);
    if (next && nw !== confirm) return fail("The two passwords don't match.");
    setBusy(true);
    setError(null);
    try {
      await onSubmit({ current: cur, next: nw });
      onClose();
    } catch (err) {
      const e2 = asError(err);
      if (e2.kind === "wrongPassword") setCur("");
      if (e2.kind !== "cancelled") fail(e2.kind === "wrongPassword" ? "Wrong password." : friendly(e2));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onClose={onClose} title={title}>
      <form onSubmit={submit} className={`flex flex-col gap-4 p-5 ${shakes ? (shakes % 2 ? "shake" : "shake2") : ""}`}>
        <div className="text-dim">{text}</div>
        {current && (
          <Field label={currentLabel} value={cur} onChange={setCur} autoComplete="current-password" invalid={!!error} />
        )}
        {next && (
          <>
            <Field label={nextLabel} value={nw} onChange={setNw} autoComplete="new-password" hint={`At least ${MIN_PASSWORD} characters.`} />
            <Field label="Confirm" value={confirm} onChange={setConfirm} autoComplete="new-password" />
          </>
        )}
        {error && <div role="alert" className="alert">{error}</div>}
        <div className="flex justify-end gap-2 pt-1">
          <button type="button" className="btn" onClick={onClose}>Cancel</button>
          <button type="submit" className="btn btn-primary" disabled={busy}>{busy ? "Working…" : submitLabel}</button>
        </div>
      </form>
    </Dialog>
  );
}

function Field({ label, value, onChange, autoComplete, hint, invalid }: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  autoComplete: string;
  hint?: string;
  invalid?: boolean;
}) {
  return (
    <label className="flex flex-col gap-1.5">
      <span className="label">{label}</span>
      <input
        type="password"
        className="field"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        autoComplete={autoComplete}
        aria-invalid={invalid || undefined}
        spellCheck={false}
      />
      {hint && <span className="text-xs text-dim">{hint}</span>}
    </label>
  );
}
