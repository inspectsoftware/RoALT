import { openUrl } from "@tauri-apps/plugin-opener";
import { asError, api, type FixKind, type HealthCheck } from "../../api";
import { t } from "../../i18n";
import { Disclosure, Skeleton } from "../../ui/Bits";
import { Button } from "../../ui/Button";
import { Icon } from "../../ui/Icon";
import { StatusDot, StatusPill } from "../../ui/Status";
import { friendly, toast, toastError } from "../../ui/Toast";
import { sk, useAct, useHealth } from "./queries";

/** RDP Wrapper's own instructions: AltDock never downloads or installs it. */
const RDP_WRAPPER = "https://github.com/stascorp/rdpwrap";

/** How a failing check gets fixed, in a line under it. */
function Fix({ fix }: { fix: FixKind | null }) {
  switch (fix) {
    case null:
      return null;
    case "setup":
      return <p className="ses-fix">{t("sessions.health.fixSetup")}</p>;
    case "restart":
      return <p className="ses-fix">{t("sessions.health.fixRestart")}</p>;
    case "rdpWrapper":
    case "rdpWrapperIni":
      return (
        <p className="ses-fix">
          <button type="button" className="linkbtn" onClick={() => openUrl(RDP_WRAPPER).catch(toastError)}>
            {t("sessions.health.fixLink")}
            <Icon name="external" size={12} />
          </button>
        </p>
      );
  }
}

function Check({ c }: { c: HealthCheck }) {
  return (
    <li className="ses-check">
      <StatusDot tone={c.ok ? "ok" : "err"} label={c.ok ? t("sessions.health.ok") : t("sessions.health.failed")} className="mt-1.5" />
      <div className="min-w-0 flex-1">
        <p className="font-medium">{c.title}</p>
        <p className="text-[13px] text-fg-2 selectable break-words">{c.detail}</p>
        {!c.ok && <Fix fix={c.fix} />}
      </div>
    </li>
  );
}

/** What background sessions need on this PC, each check with its fix, and the one "Set up" that runs
 * the elevated setup (one UAC prompt) for every session. */
export function HealthPanel({ names }: { names: string[] }) {
  const health = useHealth();
  const { busy, run } = useAct();
  const checks = health.data ?? [];
  const failing = checks.filter((c) => !c.ok);
  const passing = checks.filter((c) => c.ok);
  const setup = () => {
    let failed: string[] = [];
    void run("setup", async () => void (failed = (await api.sessionsSetup(names)).steps.filter((s) => !s.ok).map((s) => s.id)), [sk.health]).then((ok) => {
      if (!ok) return;
      if (failed.length) toast(t("sessions.health.setupFailed", { count: failed.length, steps: failed.join(", ") }), "error");
      else toast(t("sessions.health.setupDone"));
    });
  };
  // Setup can fix it only when some failing check says so (or there is nothing to say).
  const fixable = failing.some((c) => c.fix === "setup");

  return (
    <section className="panel ses-health" aria-labelledby="ses-health-title">
      <header className="ses-health-head">
        <div className="min-w-0 flex-1 flex flex-wrap items-center gap-x-3 gap-y-1">
          <h2 id="ses-health-title" className="section-title">{t("sessions.health.title")}</h2>
          {health.data &&
            (failing.length ? (
              <StatusPill tone="err" label={t("sessions.health.failing", { count: failing.length, total: checks.length })} />
            ) : (
              <StatusPill tone="ok" label={t("sessions.health.allOk", { count: checks.length })} />
            ))}
        </div>
        <Button size="md" icon="refresh" variant="ghost" busy={health.isFetching && busy !== "setup"} onClick={() => void health.refetch()}>
          {t("sessions.health.check")}
        </Button>
        <Button size="md" icon="shield" variant={fixable ? "primary" : "secondary"} busy={busy === "setup"} onClick={() => busy !== "setup" && setup()}>
          {busy === "setup" ? t("sessions.health.settingUp") : t("sessions.health.setup")}
        </Button>
      </header>
      <p className="text-[13px] text-fg-2 max-w-[72ch]">{t("sessions.health.lead")}</p>

      {health.isError ? (
        <p className="alert">{friendly(asError(health.error))}</p>
      ) : !health.data ? (
        <div className="flex flex-col gap-3 py-1">
          <Skeleton width="60%" height={14} />
          <Skeleton width="40%" height={14} />
          <span className="sr-only">{t("common.loading")}</span>
        </div>
      ) : (
        <>
          {failing.length > 0 && (
            <ul className="ses-checks">
              {failing.map((c) => <Check key={c.id} c={c} />)}
            </ul>
          )}
          {passing.length > 0 && (
            <Disclosure summary={t("sessions.health.passing", { count: passing.length })}>
              <ul className="ses-checks">
                {passing.map((c) => <Check key={c.id} c={c} />)}
              </ul>
            </Disclosure>
          )}
        </>
      )}
    </section>
  );
}
