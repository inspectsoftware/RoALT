import { useEffect, useMemo, useState } from "react";
import { encode } from "uqr";
import { api, type RemotePairPending, type RemotePairSession, type RemoteRole } from "../../api";
import { t } from "../../i18n";
import { Button } from "../../ui/Button";
import { Segmented } from "../../ui/controls";
import { Dialog } from "../../ui/Dialog";
import { toast, toastError } from "../../ui/Toast";

function QrSvg({ value, size = 200 }: { value: string; size?: number }) {
  const { modules, count } = useMemo(() => {
    const res = encode(value, { ecc: "M", border: 2 });
    return { modules: res.data, count: res.size };
  }, [value]);

  const rects: string[] = [];
  for (let y = 0; y < count; y++) {
    for (let x = 0; x < count; x++) {
      if (modules[y][x]) rects.push(`M${x},${y}h1v1h-1z`);
    }
  }
  return (
    <svg width={size} height={size} viewBox={`0 0 ${count} ${count}`} shapeRendering="crispEdges" role="img" aria-label={t("remote.qr")}>
      <rect width={count} height={count} fill="#ffffff" />
      <path d={rects.join("")} fill="#111111" />
    </svg>
  );
}

/** How the request was made (a typed code has a check code, a scanned QR code has none), who it
 * claims to be, and the check code to compare. */
function PairRequest({ pending }: { pending: RemotePairPending }) {
  return (
    <>
      <div className="remote-mode" data-mode={pending.sas ? "manual" : "qr"}>
        {pending.sas ? t("remote.modeManual") : t("remote.modeQr")}
      </div>
      <div className="remote-request">
        <span>{t("remote.requestFrom")}</span>
        <bdi className="remote-request-name">{pending.label}</bdi>
      </div>
      {pending.sas && (
        <>
          <div style={{ fontSize: 13, color: "var(--fg-2)" }}>{t("remote.sas")}</div>
          <div className="remote-sas-digits">{pending.sas}</div>
          <div style={{ fontSize: 12.5, color: "var(--attention)" }}>{t("remote.manualWarn")}</div>
        </>
      )}
    </>
  );
}

export function PairDialog({
  open,
  session,
  connected,
  onClose,
}: {
  open: boolean;
  session: RemotePairSession | null;
  connected: boolean;
  onClose: () => void;
}) {
  const [role, setRole] = useState<RemoteRole>("readonly");
  const [busy, setBusy] = useState(false);
  const [armed, setArmed] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const pending = session?.pending ?? null;

  useEffect(() => {
    if (!open || !session) return;
    setNow(Date.now());
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [open, session]);

  useEffect(() => {
    setRole("readonly");
    setArmed(false);
    const id = window.setTimeout(() => setArmed(true), 1000);
    return () => window.clearTimeout(id);
  }, [pending?.viewerId]);

  const secsLeft = session ? Math.max(0, Math.ceil((session.expiresAt - now) / 1000)) : 0;
  const mm = Math.floor(secsLeft / 60);
  const ss = String(secsLeft % 60).padStart(2, "0");

  const handleClose = () => {
    if (session) api.remotePairCancel().catch(() => {});
    onClose();
  };

  useEffect(() => {
    if (!open || !session || secsLeft > 0) return;
    if (pending?.approved) onClose();
    else handleClose();
  }, [open, session, secsLeft]);

  const copyCode = async () => {
    if (!session) return;
    try {
      await navigator.clipboard.writeText(session.manualCode);
      toast(t("remote.codeCopied"));
    } catch {
      toast(t("remote.copyFailed"));
    }
  };

  const decide = async (approve: boolean) => {
    if (!pending) return;
    setBusy(true);
    try {
      await api.remotePairApprove(pending.viewerId, approve, role, pending.sas);
      if (!approve) onClose();
    } catch (e) {
      toastError(e);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={open} onClose={handleClose} title={t("remote.pairTitle")} width={440}>
      <div className="dlg-body">
        {!session ? (
          <div className="text-fg-2">{t("remote.pairStarting")}</div>
        ) : session.aborted ? (
          <div role="alert" className="notice" data-tone="attention">
            {t("remote.aborted")}
          </div>
        ) : pending?.approved ? (
          <div className="remote-sas-card">
            <PairRequest pending={pending} />
            <div style={{ fontSize: 13, color: "var(--fg-2)" }}>{t("remote.waitingDevice")}</div>
            <div style={{ fontSize: 12, color: "var(--fg-3)" }}>{t("remote.expires", { time: `${mm}:${ss}` })}</div>
          </div>
        ) : pending ? (
          <div className="remote-sas-card">
            <PairRequest pending={pending} />
            {!pending.sas && <div style={{ fontSize: 13, color: "var(--fg-2)" }}>{t("remote.qrRequest")}</div>}
            <div style={{ width: "100%", display: "flex", justifyContent: "center" }}>
              <Segmented
                label={t("remote.permission")}
                value={role}
                options={[
                  { value: "readonly", label: t("remote.roleView") },
                  { value: "control", label: t("remote.roleControl") },
                ]}
                onChange={(v) => setRole(v as RemoteRole)}
              />
            </div>
            <div style={{ fontSize: 11.5, color: "var(--fg-3)" }}>{t("remote.secretsStay")}</div>
            <div style={{ display: "flex", gap: 8, marginTop: 4 }}>
              <Button size="md" disabled={busy} onClick={() => void decide(false)}>
                {t("remote.deny")}
              </Button>
              <Button size="md" variant="primary" disabled={busy || !armed} onClick={() => void decide(true)}>
                {t("remote.approve")}
              </Button>
            </div>
          </div>
        ) : !connected ? (
          <div role="status" className="text-fg-2">
            {t("remote.pairReconnecting")}
          </div>
        ) : (
          <div className="remote-qr-wrap">
            <div role="note" className="notice" data-tone="attention">
              {t("remote.warnShare")}
            </div>
            <div className="remote-qr-box">
              <QrSvg value={session.qrUrl} size={196} />
            </div>
            <div style={{ fontSize: 13, color: "var(--fg-2)", textAlign: "center" }}>{t("remote.scan")}</div>
            <div className="remote-code-box">
              <span>{session.manualCode}</span>
              <Button size="sm" onClick={() => void copyCode()}>
                {t("common.copy")}
              </Button>
            </div>
            <div style={{ fontSize: 12, color: "var(--fg-3)", textAlign: "center" }}>{t("remote.expires", { time: `${mm}:${ss}` })}</div>
          </div>
        )}
      </div>
      <div className="dlg-foot">
        <Button size="md" onClick={handleClose}>
          {t("common.close")}
        </Button>
      </div>
    </Dialog>
  );
}
