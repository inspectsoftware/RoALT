import "./remote.css";
import { useEffect } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api, events } from "../../api";
import { requestNavigation } from "../../app/navigation";
import { qk, useRemote } from "../../data";
import { fmtList, t } from "../../i18n";
import { toast, toastError } from "../../ui/Toast";

/** Always-visible title-bar pill while a remote browser is connected, watching or paused, with the
 * way to the device list (to revoke one) next to it; also tells how every pairing ended. */
export function RemoteBadge() {
  const qc = useQueryClient();
  const remote = useRemote().data;

  useEffect(() => {
    const off = events.onRemotePairing((e) => {
      void qc.invalidateQueries({ queryKey: qk.remote });
      if (e.outcome === "paired") toast(t("remote.paired", { name: e.label ?? "" }));
      else if (e.outcome === "expired") toast(t("remote.pairExpired", { name: e.label ?? "" }), "error");
      else toast(t("remote.pairLeft"), "error");
    });
    return () => void off.then((f) => f());
  }, [qc]);

  if (!remote) return null;
  const paused = remote.paused && remote.state !== "off";
  if (!paused && remote.activeViewers <= 0) return null;

  const setPaused = (next: boolean) => {
    api.remoteSetPaused(next).then(
      () => {
        void qc.invalidateQueries({ queryKey: qk.remote });
        toast(next ? t("remote.paused") : t("remote.resumed"));
      },
      toastError,
    );
  };

  const devices = (
    <button type="button" className="remote-pill" onClick={() => requestNavigation("settings", "remote")}>
      {t("remote.devices", { n: remote.devices.length })}
    </button>
  );

  if (paused) {
    return (
      <>
        <button type="button" className="remote-pill is-paused" data-tip={t("remote.pillPausedTip")} onClick={() => setPaused(false)}>
          <span className="remote-dot is-warn" />
          <span>{t("remote.pillPaused")}</span>
        </button>
        {devices}
      </>
    );
  }

  const names = fmtList(remote.devices.filter((d) => d.connected).map((d) => d.label)) || remote.activeViewers;
  const watching = remote.watching > 0;
  return (
    <>
      <button
        type="button"
        className={`remote-pill${watching ? " is-watching" : ""}`}
        data-tip={watching ? t("remote.pillWatchingTip", { names }) : t("remote.pillTip", { names })}
        onClick={() => setPaused(true)}
      >
        <span className={`remote-dot ${watching ? "is-err" : "is-on"}`} />
        <span>{watching ? t("remote.pillWatching", { n: remote.watching }) : t("remote.pill", { n: remote.activeViewers })}</span>
      </button>
      {devices}
    </>
  );
}
