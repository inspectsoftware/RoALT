import { useEffect, useState } from "react";
import { asError, api, type AccountSummary, type SessionConfig, type SessionView } from "../../api";
import { t } from "../../i18n";
import { Chip } from "../../ui/Bits";
import { Button, IconButton } from "../../ui/Button";
import { Input } from "../../ui/Field";
import { StatusPill, type Tone } from "../../ui/Status";
import { friendly, toast } from "../../ui/Toast";
import { accountName } from "../accounts/shared";
import { ClientCard } from "./ClientCard";
import { controlOk, sk, useAct, useRemoteMacros, useRemoteState } from "./queries";

/** How long after Start a session that doesn't answer yet counts as signing in, not as broken. */
const SIGN_IN_MS = 60_000;

function badge(s: SessionView, startedAt: number | undefined): { tone: Tone; label: string; pulse?: boolean; tip?: string } {
  if (s.wts === "none") return { tone: "neutral", label: t("sessions.state.off") };
  if (s.wts === "disconnected") return { tone: "info", label: t("sessions.state.disconnected"), tip: t("sessions.state.disconnectedHint") };
  if (s.control.kind === "ok") return { tone: "ok", label: t("sessions.state.running") };
  if (startedAt && Date.now() - startedAt < SIGN_IN_MS) return { tone: "info", label: t("sessions.state.signingIn"), pulse: true };
  return { tone: "attention", label: t("sessions.state.noAnswer"), tip: s.control.kind === "down" ? s.control.reason : undefined };
}

/** The launch box: a target and which of the session's accounts (this app's ids; those not
 * running there yet, picked by default). */
function LaunchBox({ s, accounts, running }: { s: SessionView; accounts: Map<number, AccountSummary>; running: Set<number> }) {
  const name = s.config.name;
  const { busy, run } = useAct();
  const [target, setTarget] = useState("");
  const [off, setOff] = useState<Set<number>>(new Set());
  const ids = s.config.accountIds.filter((id) => !running.has(id));
  const picked = ids.filter((id) => !off.has(id));
  const launch = () => {
    if (!target.trim() || !picked.length || busy) return;
    let started = 0;
    void run("launch", async () => void ({ started } = await api.remoteLaunch(name, picked, target.trim())), [sk.state(name)]).then(
      (ok) => ok && toast(t("sessions.launch.started", { count: started, name })),
    );
  };
  return (
    <form
      className="ses-launch"
      onSubmit={(e) => {
        e.preventDefault();
        launch();
      }}
    >
      <Input leading="play" aria-label={t("sessions.launch.target")} placeholder={t("sessions.launch.placeholder")} value={target} onChange={(e) => setTarget(e.target.value)} />
      <div className="flex flex-wrap items-center gap-1.5 min-w-0" role="group" aria-label={t("sessions.launch.accounts")}>
        {s.config.accountIds.length === 0 && <span className="text-[13px] text-fg-3">{t("sessions.launch.noAccounts")}</span>}
        {s.config.accountIds.map((id) => {
          const a = accounts.get(id);
          const isRunning = running.has(id);
          return (
            <Chip
              key={id}
              small
              dot={a?.color ?? undefined}
              pressed={!isRunning && !off.has(id)}
              disabled={isRunning}
              data-tip={isRunning ? t("sessions.launch.alreadyRunning") : undefined}
              onClick={() =>
                setOff((cur) => {
                  const next = new Set(cur);
                  if (next.has(id)) next.delete(id);
                  else next.add(id);
                  return next;
                })
              }
            >
              {a ? accountName(a) : t("dashboard.account", { id })}
            </Chip>
          );
        })}
      </div>
      <Button type="submit" size="md" variant="primary" icon="play" busy={busy === "launch"} disabled={!target.trim() || !picked.length}>
        {t("sessions.launch.button", { count: picked.length })}
      </Button>
    </form>
  );
}

/** What runs in a session whose AltDock answers: launch box, push and install, the client cards. */
function Live({ s, accounts, now, onPushMacros }: { s: SessionView; accounts: Map<number, AccountSummary>; now: number; onPushMacros: () => void }) {
  const name = s.config.name;
  const health = s.control.kind === "ok" ? s.control.health : null;
  const state = useRemoteState(name, true);
  const macros = useRemoteMacros(name, true).data ?? [];
  const { busy, run } = useAct();
  const clients = state.data ?? [];
  // The session's account ids are its own: its clients match this app's accounts by Roblox user.
  const byUser = new Map([...accounts.values()].map((a) => [a.userId, a]));
  const running = new Set(clients.filter((c) => c.state !== "exited").flatMap((c) => byUser.get(c.userId)?.id ?? []));
  const pushAccounts = () => {
    let imported = 0;
    void run("accounts", async () => void ({ imported } = await api.sessionPushAccounts(name, s.config.accountIds))).then(
      (ok) => ok && toast(t("sessions.push.accountsDone", { count: imported, name })),
    );
  };
  const install = () => void run("install", () => api.remoteRobloxInstall(name)).then((ok) => ok && toast(t("sessions.install.done", { name })));

  return (
    <>
      {health?.vaultLocked && <p className="alert">{t("sessions.vaultLocked")}</p>}
      {health && !health.robloxInstalled && (
        <div className="notice" data-tone="attention">
          <p className="flex-[1_1_240px] min-w-0 text-fg-2">{t("sessions.install.missing")}</p>
          <Button size="md" variant="primary" icon="download" busy={busy === "install"} onClick={install}>
            {busy === "install" ? t("sessions.install.installing") : t("sessions.install.button")}
          </Button>
        </div>
      )}
      <div className="ses-tools">
        <LaunchBox s={s} accounts={accounts} running={running} />
        <div className="ses-tools-end">
          <Button size="md" icon="upload" busy={busy === "accounts"} disabled={!s.config.accountIds.length} data-tip={t("sessions.push.accountsHint")} onClick={pushAccounts}>
            {t("sessions.push.accounts")}
          </Button>
          <Button size="md" icon="upload" data-tip={t("sessions.push.macrosHint")} onClick={onPushMacros}>
            {t("sessions.push.macros")}
          </Button>
          <Button size="md" variant="ghost" icon="stop" busy={busy === "kill"} disabled={!clients.length} onClick={() => void run("kill", () => api.remoteKillAll(name), [sk.state(name)])}>
            {t("sessions.killAll")}
          </Button>
        </div>
      </div>
      {state.isError ? (
        <div className="notice" data-tone="attention">
          <p className="alert flex-[1_1_240px] min-w-0">{friendly(asError(state.error))}</p>
          <Button size="md" icon="refresh" busy={state.isFetching} onClick={() => void state.refetch()}>{t("common.tryAgain")}</Button>
        </div>
      ) : state.data && clients.length === 0 ? (
        <div className="empty ses-empty">
          <p>{t("sessions.noClients")}</p>
        </div>
      ) : (
        <div className="ses-grid">
          {clients.map((c) => (
            <ClientCard key={c.instanceId} session={name} c={c} account={byUser.get(c.userId)} macros={macros} now={now} />
          ))}
        </div>
      )}
    </>
  );
}

/** One session: its name, state and actions, then (while its AltDock answers) what runs in it. */
export function SessionPanel({ s, accounts, now, onEdit, onRemove, onPushMacros }: {
  s: SessionView;
  accounts: Map<number, AccountSummary>;
  now: number;
  onEdit: (c: SessionConfig) => void;
  onRemove: (c: SessionConfig) => void;
  onPushMacros: (name: string) => void;
}) {
  const c = s.config;
  const { busy, run } = useAct();
  const [startedAt, setStartedAt] = useState<number>();
  // Answering ends the signing-in grace.
  useEffect(() => {
    if (s.control.kind === "ok") setStartedAt(undefined);
  }, [s.control.kind]);
  const b = badge(s, startedAt);
  const live = controlOk(s);
  const start = () =>
    void run("start", () => api.sessionStart(c.name), [sk.health]).then((ok) => ok && setStartedAt(Date.now()));
  const idOf = `ses-${c.name.replace(/\W/g, "_")}`;

  return (
    <section className="panel ses-panel" aria-labelledby={idOf}>
      <header className="ses-head">
        <div className="min-w-0 flex-1 flex flex-col gap-1">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-1 min-w-0">
            <h2 id={idOf} className="section-title truncate">{c.name}</h2>
            <span data-tip={b.tip}>
              <StatusPill tone={b.tone} pulse={b.pulse} label={b.label} />
            </span>
            <span className="tag">{c.mode === "shared" ? t("sessions.mode.shared") : t("sessions.mode.dedicated")}</span>
          </div>
          <p className="meta mono truncate">
            {t("sessions.meta", { user: c.windowsUser, address: c.address, w: c.width, h: c.height, count: c.accountIds.length })}
          </p>
        </div>
        <div className="ses-actions">
          {s.wts !== "active" && (
            <Button size="md" variant="primary" icon="play" busy={busy === "start"} onClick={start}>
              {s.wts === "disconnected" ? t("sessions.reconnect") : t("sessions.start")}
            </Button>
          )}
          {s.wts === "active" && (
            <Button size="md" icon={s.windowVisible ? "eyeOff" : "eye"} busy={busy === "window"} onClick={() => void run("window", () => api.sessionWindow(c.name, !s.windowVisible))}>
              {s.windowVisible ? t("sessions.hideWindow") : t("sessions.showWindow")}
            </Button>
          )}
          {s.wts === "active" && (
            <Button size="md" busy={busy === "disconnect"} data-tip={t("sessions.disconnectHint")} onClick={() => void run("disconnect", () => api.sessionDisconnect(c.name))}>
              {t("sessions.disconnect")}
            </Button>
          )}
          {s.wts !== "none" && (
            <Button size="md" icon="signin" busy={busy === "logoff"} disabled={s.control.kind !== "ok"} data-tip={t("sessions.logoffHint")} onClick={() => void run("logoff", () => api.sessionLogoff(c.name), [sk.state(c.name)])}>
              {t("sessions.logoff")}
            </Button>
          )}
          <IconButton icon="pencil" label={t("sessions.edit", { name: c.name })} onClick={() => onEdit(c)} />
          <IconButton icon="trash" label={t("sessions.remove.button", { name: c.name })} onClick={() => onRemove(c)} />
        </div>
      </header>

      {live ? (
        <Live s={s} accounts={accounts} now={now} onPushMacros={() => onPushMacros(c.name)} />
      ) : s.wts === "none" ? (
        <p className="text-[13px] text-fg-3">{t("sessions.offHint")}</p>
      ) : s.control.kind === "down" && b.tone === "attention" ? (
        <p className="alert text-[13px]">{t("sessions.controlDown", { error: s.control.reason })}</p>
      ) : null}
    </section>
  );
}
