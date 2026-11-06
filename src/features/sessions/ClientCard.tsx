import { useRef, useState } from "react";
import { api, type AccountSummary, type AfkAction, type AfkClientConfig, type ControlClient, type Macro } from "../../api";
import { t } from "../../i18n";
import { Button, IconButton } from "../../ui/Button";
import { NumberField, Switch, TextField } from "../../ui/controls";
import { Select } from "../../ui/Field";
import { StatusPill } from "../../ui/Status";
import { accountName, Avatar } from "../accounts/shared";
import { STATE_TONE } from "../dashboard/homeLayout";
import { STATES, uptime } from "../launch/Running";
import { mb, pct } from "../monitor/MonitorView";
import { sk, useAct, useRemoteThumbnail } from "./queries";

type ActionChoice = "f13" | "space" | "jump" | "jumpTurn" | "key";
const ACTIONS: ActionChoice[] = ["f13", "space", "jump", "jumpTurn", "key"];
const choiceOf = (a: AfkAction): ActionChoice => (typeof a === "object" ? "key" : a);

/** Anti-AFK for this client's account: on/off, how often (1–19 min) and which key. A custom key
 * shows its field first and is only sent once typed. */
function AfkControls({ session, c, act }: { session: string; c: ControlClient; act: ReturnType<typeof useAct> }) {
  const [customOpen, setCustomOpen] = useState(false);
  const cfg = c.afk;
  const send = (next: AfkClientConfig) => act.run("afk", () => api.remoteAfk(session, c.accountId, next), [sk.state(session)]);
  const choice: ActionChoice = customOpen ? "key" : choiceOf(cfg.action);
  const key = typeof cfg.action === "object" ? cfg.action.key : "";
  const label = t("sessions.client.afk");
  return (
    <div className="ses-row">
      <div className="flex items-center gap-2 min-w-0">
        <Switch checked={cfg.enabled} label={t("sessions.client.afkFor", { name: c.username })} onChange={(on) => void send({ ...cfg, enabled: on })} />
        <span className="ses-row-label">{label}</span>
      </div>
      <div className="ses-row-controls" data-off={!cfg.enabled || undefined} data-custom={choice === "key" || undefined}>
        <NumberField
          name={t("sessions.client.interval")}
          unit={t("sessions.client.minutes")}
          short={t("sessions.client.min")}
          min={1}
          max={19}
          value={Math.round(cfg.intervalSec / 60)}
          onCommit={(n) => void send({ ...cfg, intervalSec: n * 60 })}
          className="w-14 ses-num"
        />
        <Select<ActionChoice>
          aria-label={t("sessions.client.action")}
          value={choice}
          className="ses-select"
          options={ACTIONS.map((a) => ({ value: a, label: t(`sessions.afk.${a}`) }))}
          onChange={(v) => {
            if (v === "key") return setCustomOpen(true);
            setCustomOpen(false);
            void send({ ...cfg, action: v });
          }}
        />
        {choice === "key" && (
          <TextField
            label={t("sessions.client.key")}
            value={key}
            placeholder={t("sessions.client.keyPlaceholder")}
            className="ses-key"
            onCommit={(k) => {
              if (!k) return Promise.reject(new Error("empty"));
              return send({ ...cfg, action: { key: k } }).then((ok) => {
                if (!ok) throw new Error("refused");
                setCustomOpen(false);
              });
            }}
          />
        )}
      </div>
    </div>
  );
}

/** The macro assigned to this client's account, whether it starts by itself in a game, and Run/Stop. */
function MacroControls({ session, c, macros, act }: { session: string; c: ControlClient; macros: Macro[]; act: ReturnType<typeof useAct> }) {
  const autorun = c.macroAutorun;
  const assign = (macroId: number | null, auto: boolean) =>
    void act.run("assign", () => api.remoteMacroAssign(session, c.accountId, macroId, auto), [sk.state(session)]);
  const running = c.macroRun;
  const inGame = c.state === "inGame";
  const none = "none";
  return (
    <div className="ses-row">
      <div className="flex items-center gap-2 min-w-0">
        <Switch checked={autorun && c.macroAssigned !== null} disabled={c.macroAssigned === null} label={t("sessions.client.autorunFor", { name: c.username })} onChange={(on) => assign(c.macroAssigned, on)} />
        <span className="ses-row-label" data-tip={t("sessions.client.autorunHint")}>{t("sessions.client.autorun")}</span>
      </div>
      <div className="ses-row-controls">
        <Select<string>
          aria-label={t("sessions.client.macro")}
          value={c.macroAssigned === null ? none : String(c.macroAssigned)}
          className="ses-select ses-macro"
          options={[{ value: none, label: macros.length ? t("sessions.client.noMacro") : t("sessions.client.noMacros") }, ...macros.map((m) => ({ value: String(m.id), label: m.name }))]}
          onChange={(v) => assign(v === none ? null : Number(v), c.macroAssigned === null ? true : autorun)}
        />
        {running ? (
          <Button size="sm" icon="stop" busy={act.busy === "stop"} onClick={() => void act.run("stop", () => api.remoteMacroStop(session, c.instanceId), [sk.state(session)])}>
            {t("sessions.client.stop")}
          </Button>
        ) : (
          <Button
            size="sm"
            icon="play"
            busy={act.busy === "run"}
            disabled={c.macroAssigned === null || !inGame}
            data-tip={!inGame ? t("sessions.client.runInGame") : undefined}
            onClick={() => c.macroAssigned !== null && void act.run("run", () => api.remoteMacroRun(session, c.macroAssigned!, [c.instanceId]), [sk.state(session)])}
          >
            {t("sessions.client.run")}
          </Button>
        )}
      </div>
    </div>
  );
}

/** One client in a session: its picture (about once a second), state, close, anti-AFK and macro. */
export function ClientCard({ session, c, account, macros, now }: {
  session: string;
  c: ControlClient;
  account: AccountSummary | undefined;
  macros: Macro[];
  now: number;
}) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const { status, painted } = useRemoteThumbnail(session, c.instanceId, canvas);
  const act = useAct();
  const name = account ? accountName(account) : c.displayName || c.username;
  const closing = c.state === "exited" || act.busy === "close";
  const wait = status === "noWindow" || (status === null && c.state === "starting") ? t("dashboard.noWindow") : status === "pending" || status === null ? t("dashboard.waitingPicture") : status === "unsupported" ? t("dashboard.noPreview") : "";
  const pill = closing
    ? { tone: "neutral" as const, label: t("launch.running.closing"), pulse: true }
    : c.state !== "inGame"
      ? { tone: STATE_TONE[c.state], label: STATES[c.state][0], pulse: c.state !== "disconnected" }
      : null;

  return (
    <div role="group" aria-label={t("dashboard.cardLabel", { name, state: pill?.label ?? STATES[c.state][0] })} className="panel home-card ses-card">
      <div className="home-pic ses-pic" data-painted={painted || undefined}>
        <canvas ref={canvas} aria-hidden="true" />
        {!painted && (
          <span className="home-pic-wait" data-breathe={status === null || status === "pending" || status === "noWindow" || undefined}>
            {wait}
          </span>
        )}
        {pill && (
          <div className="home-pill">
            <StatusPill tone={pill.tone} pulse={pill.pulse} label={pill.label} />
          </div>
        )}
        <div className="ses-close">
          <IconButton icon="x" size="sm" label={t("sessions.client.close", { name })} disabled={closing} onClick={(e) => void act.run("close", () => api.remoteClose(session, c.instanceId, e.shiftKey), [sk.state(session)])} />
        </div>
      </div>
      <div className="home-info">
        <div className="flex items-center gap-2.5 min-w-0">
          {account && <Avatar account={account} size={32} />}
          <div className="flex-1 min-w-0 flex flex-col">
            <span className="font-medium truncate">{name}</span>
            <span className="flex gap-1 text-[13px] text-fg-2 min-w-0">
              <span className="truncate">{c.placeName ?? t("dashboard.noGameYet")}</span>
              <span className="flex-none tabular-nums">· {uptime(c.startedAt, now)}</span>
            </span>
          </div>
        </div>
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-[12px] text-fg-3 tabular-nums">
          <span>{t("home.cpu")} <span className="text-fg font-medium">{pct(c.cpuPct)}</span></span>
          <span>{t("home.memory")} <span className="text-fg font-medium">{c.memoryMb === null ? "—" : mb(c.memoryMb)}</span></span>
          {c.macroRun && (
            <StatusPill tone="ok" pulse label={t("sessions.client.macroRunning", { name: c.macroRun.name })} detail={uptime(c.macroRun.startedAt, now)} className="ms-auto" />
          )}
        </div>
        <div className="ses-controls">
          <AfkControls session={session} c={c} act={act} />
          <MacroControls session={session} c={c} macros={macros} act={act} />
        </div>
      </div>
    </div>
  );
}
