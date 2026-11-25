import "./remote.css";
import { useEffect, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api, type RemoteRole, type RemoteState } from "../../api";
import { qk, useRemote, useSetSettings, useSettings } from "../../data";
import { t } from "../../i18n";
import { Button } from "../../ui/Button";
import { Segmented, Switch } from "../../ui/controls";
import { ConfirmDialog } from "../../ui/Dialog";
import { Input } from "../../ui/Field";
import { toast, toastError } from "../../ui/Toast";
import { ago } from "../accounts/shared";
import { SettingSection } from "../settings/kit/SettingSection";
import { PairDialog } from "./PairDialog";

function statusLabel(state: RemoteState, active: number, error: string | null, paused: boolean): { text: string; tone: "on" | "warn" | "err" | "off" } {
  if (paused && state !== "off") return { text: t("remote.status.pausedByUser"), tone: "warn" };
  switch (state) {
    case "off":
      return { text: t("remote.status.off"), tone: "off" };
    case "idle":
      return { text: t("remote.status.idle"), tone: "off" };
    case "connecting":
      return { text: t("remote.status.connecting"), tone: "warn" };
    case "connected":
      return { text: active > 0 ? t("remote.status.viewing", { n: active }) : t("remote.status.connected"), tone: "on" };
    case "pausedPublic":
      return { text: t("remote.status.paused"), tone: "warn" };
    case "error":
      return { text: error ? t("remote.status.error", { why: error }) : t("remote.status.retrying"), tone: "err" };
  }
}

export function RemotePanel({ block }: { block?: string }) {
  const qc = useQueryClient();
  const settings = useSettings().data;
  const setSettings = useSetSettings();
  const remote = useRemote().data;

  const [pcName, setPcName] = useState("");
  const [relayUrl, setRelayUrl] = useState("");
  const [showAdvanced, setShowAdvanced] = useState(false);
  const [pairOpen, setPairOpen] = useState(false);
  const [startingPair, setStartingPair] = useState(false);
  const [confirmRelay, setConfirmRelay] = useState(false);

  useEffect(() => {
    if (!settings) return;
    setPcName(settings.remote.pcName);
    setRelayUrl(settings.remote.relayUrl);
  }, [settings?.remote.pcName, settings?.remote.relayUrl]);

  useEffect(() => {
    if (pairOpen && remote && !remote.pairing && !startingPair) {
      setPairOpen(false);
    }
  }, [pairOpen, remote?.pairing, startingPair]);

  if (!settings) return null;
  const r = settings.remote;
  const paused = remote?.paused ?? false;
  const info = statusLabel(remote?.state ?? (r.enabled ? "connecting" : "off"), remote?.activeViewers ?? 0, remote?.error ?? null, paused);

  const saveRemote = (patch: Partial<typeof r>) => {
    setSettings.mutate({ remote: { ...r, ...patch } }, { onSuccess: () => void qc.invalidateQueries({ queryKey: qk.remote }) });
  };

  const startPair = async () => {
    if ((remote?.devices.length ?? 0) >= 64) {
      toast(t("remote.tooMany"), "error");
      return;
    }
    setStartingPair(true);
    setPairOpen(true);
    try {
      await api.remotePairStart();
      await qc.invalidateQueries({ queryKey: qk.remote });
    } catch (e) {
      setPairOpen(false);
      toastError(e);
    } finally {
      setStartingPair(false);
    }
  };

  const setPaused = (next: boolean) => {
    api.remoteSetPaused(next).then(() => {
      void qc.invalidateQueries({ queryKey: qk.remote });
      toast(next ? t("remote.paused") : t("remote.resumed"));
    }, toastError);
  };

  const revoke = async (id: string, label: string) => {
    try {
      await api.remoteDeviceRevoke(id);
      await qc.invalidateQueries({ queryKey: qk.remote });
      toast(t("remote.revoked", { name: label }));
    } catch (e) {
      toastError(e);
    }
  };

  const setRole = async (id: string, role: RemoteRole) => {
    try {
      await api.remoteDeviceUpdate(id, undefined, role);
      await qc.invalidateQueries({ queryKey: qk.remote });
    } catch (e) {
      toastError(e);
    }
  };

  return (
    <SettingSection sub label="settings.section.remote" block={block}>
      <div className="remote-card">
        <div className="remote-head">
          <div>
            <div style={{ fontSize: 14, fontWeight: 600, color: "var(--fg)" }}>{t("remote.title")}</div>
            <div style={{ fontSize: 12.5, color: "var(--fg-2)", marginTop: 2 }}>{t("remote.text")}</div>
          </div>
          <Switch label={t("remote.title")} checked={r.enabled} onChange={(enabled) => saveRemote({ enabled })} />
        </div>

        <div className="remote-status-row">
          <span className={`remote-dot ${info.tone === "on" ? "is-on" : info.tone === "warn" ? "is-warn" : info.tone === "err" ? "is-err" : ""}`} />
          <span>{info.text}</span>
          {paused ? (
            <Button size="sm" onClick={() => setPaused(false)}>
              {t("remote.resume")}
            </Button>
          ) : (
            r.enabled && (
              <Button size="sm" onClick={() => setPaused(true)}>
                {t("remote.pause")}
              </Button>
            )
          )}
        </div>

        <div className="remote-fields">
          <div className="remote-field">
            <label htmlFor="remote-pc-name">{t("remote.pcName")}</label>
            <Input
              id="remote-pc-name"
              value={pcName}
              placeholder={remote?.pcName || "My PC"}
              onChange={(e) => setPcName(e.target.value)}
              onBlur={() => {
                if (pcName.trim() !== r.pcName) saveRemote({ pcName: pcName.trim() });
              }}
            />
          </div>
          <div className="remote-field" style={{ justifyContent: "flex-end" }}>
            <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 10, paddingTop: 18 }}>
              <span style={{ fontSize: 12.5, color: "var(--fg-2)" }}>{t("remote.pausePublic")}</span>
              <Switch label={t("remote.pausePublic")} checked={r.blockOnPublicNetwork} onChange={(blockOnPublicNetwork) => saveRemote({ blockOnPublicNetwork })} />
            </div>
            <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 10 }}>
              <span style={{ fontSize: 12.5, color: "var(--fg-2)" }}>{t("remote.viewPreviews")}</span>
              <Switch label={t("remote.viewPreviews")} checked={r.allowViewPreviews} onChange={(allowViewPreviews) => saveRemote({ allowViewPreviews })} />
            </div>
          </div>
        </div>

        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8, paddingTop: 4 }}>
          <div style={{ fontSize: 13, fontWeight: 600, color: "var(--fg)" }}>{t("remote.devices", { n: remote?.devices.length ?? 0 })}</div>
          <div style={{ display: "flex", gap: 8 }}>
            {remote?.customRelay && (
              <Button size="sm" onClick={() => setShowAdvanced((v) => !v)}>
                {showAdvanced ? t("remote.relayHide") : t("remote.relay")}
              </Button>
            )}
            <Button size="sm" variant="primary" onClick={() => void startPair()}>
              {t("remote.pair")}
            </Button>
          </div>
        </div>

        {showAdvanced && remote?.customRelay && (
          <div className="remote-field">
            <label htmlFor="remote-relay-url">{t("remote.relayLabel")}</label>
            <div style={{ display: "flex", gap: 8 }}>
              <Input id="remote-relay-url" value={relayUrl} onChange={(e) => setRelayUrl(e.target.value)} />
              <Button size="sm" onClick={() => setConfirmRelay(true)} disabled={relayUrl.trim() === r.relayUrl}>
                {t("common.save")}
              </Button>
            </div>
          </div>
        )}

        <div className="remote-devices">
          {!remote || remote.devices.length === 0 ? (
            <div style={{ fontSize: 12.5, color: "var(--fg-3)", padding: "6px 0" }}>{t("remote.none")}</div>
          ) : (
            remote.devices.map((d) => (
              <div key={d.id} className="remote-device">
                <div className="remote-device-meta">
                  <div className="remote-device-name">
                    <span className={`remote-dot ${d.connected ? "is-on" : ""}`} />
                    <span>{d.label}</span>
                  </div>
                  <div className="remote-device-sub">{d.connected ? t("remote.viewingNow") : d.pending ? t("remote.waitingFirst") : t("remote.lastSeen", { when: ago(d.lastSeenAt) })}</div>
                </div>
                <div className="remote-device-actions">
                  <Segmented
                    label={t("remote.roleFor", { name: d.label })}
                    value={d.role}
                    options={[
                      { value: "control", label: t("remote.roleControl") },
                      { value: "readonly", label: t("remote.roleView") },
                    ]}
                    onChange={(v) => void setRole(d.id, v as RemoteRole)}
                  />
                  <Button size="sm" variant="danger" onClick={() => void revoke(d.id, d.label)}>
                    {t("remote.revoke")}
                  </Button>
                </div>
              </div>
            ))
          )}
        </div>
      </div>

      <PairDialog open={pairOpen} session={remote?.pairing ?? null} connected={remote?.state === "connected"} onClose={() => setPairOpen(false)} />
      <ConfirmDialog
        open={confirmRelay}
        onClose={() => setConfirmRelay(false)}
        onConfirm={() => {
          setConfirmRelay(false);
          saveRemote({ relayUrl: relayUrl.trim() });
        }}
        title={t("remote.relayConfirmTitle")}
        text={t("remote.relayConfirmText", { url: relayUrl.trim() || "wss://remote.roalt.dev/ws/remote" })}
        confirmLabel={t("remote.relayConfirm")}
      />
    </SettingSection>
  );
}
