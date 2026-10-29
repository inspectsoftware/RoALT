import { useState, type FormEvent } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type DiscordEvents, type Settings } from "../../api";
import { qk, useSetSettings } from "../../data";
import { NumberField, Switch } from "../../ui/controls";
import { toast, toastError } from "../../ui/Toast";
import { Heading, Row } from "../settings/SettingsView";
import { t, type Key } from "../../i18n";

const EVENTS: [key: Exclude<keyof DiscordEvents, "statusEveryMin">, title: Key, hint: Key][] = [
  ["launch", "discord.evLaunch", "discord.evLaunchHint"],
  ["crash", "discord.evCrash", "discord.evCrashHint"],
  ["hang", "discord.evHang", "discord.evHangHint"],
  ["disconnect", "discord.evDisconnect", "discord.evDisconnectHint"],
  ["restart", "discord.evRestart", "discord.evRestartHint"],
  ["memRestart", "discord.evMemRestart", "discord.evMemRestartHint"],
  ["breaker", "discord.evBreaker", "discord.evBreakerHint"],
  ["batchDone", "discord.evBatchDone", "discord.evBatchDoneHint"],
];

/** Alerts to a Discord channel through a webhook; the URL itself stays in the vault. */
export function DiscordView({ settings }: { settings: Settings }) {
  const qc = useQueryClient();
  const set = useSetSettings();
  const status = useQuery({ queryKey: qk.discord, queryFn: api.discordGet }).data;
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState<"save" | "test" | "remove" | null>(null);
  const events = settings.discord;
  const setEvents = (patch: Partial<DiscordEvents>) => set.mutate({ discord: { ...events, ...patch } });

  const save = (e: FormEvent) => {
    e.preventDefault();
    if (!url.trim() || busy) return;
    setBusy("save");
    api
      .discordSet(url.trim())
      .then((st) => {
        qc.setQueryData(qk.discord, st);
        setUrl("");
        toast(t("discord.saved"));
      }, toastError)
      .finally(() => setBusy(null));
  };
  const remove = () => {
    setBusy("remove");
    api
      .discordSet(null)
      .then((st) => {
        qc.setQueryData(qk.discord, st);
        toast(t("discord.removed"));
      }, toastError)
      .finally(() => setBusy(null));
  };
  const test = () => {
    setBusy("test");
    api
      .discordTest()
      .then(() => toast(t("discord.testSent")), toastError)
      .finally(() => setBusy(null));
  };

  return (
    <main className="scroll flex-1 min-w-0 px-10 pt-6 pb-12">
      <div className="enter max-w-[760px] flex flex-col">
        <h1 className="text-xl font-semibold tracking-[-0.01em] mb-2">Discord</h1>
        <p className="text-dim max-w-[620px]">
          {t("discord.intro")}
        </p>

        <Heading id="discord-webhook">{t("discord.webhook")}</Heading>
        <Row
          title={t("discord.webhook")}
          hint={
            status?.configured && !status.webhook
              ? t("discord.setLocked")
              : status?.configured
              ? t("discord.setId", { id: status.webhook ?? t("discord.unknown") })
              : t("discord.notSet")
          }
        >
          {status?.configured && (
            <div className="flex gap-2">
              <button className="btn" disabled={busy !== null} onClick={test}>{busy === "test" ? t("discord.sending") : t("discord.sendTest")}</button>
              <button className="btn" disabled={busy !== null} onClick={remove}>{t("common.remove")}</button>
            </div>
          )}
        </Row>
        <form onSubmit={save} className="flex flex-wrap items-center gap-2 py-4 border-b border-line">
          <input
            type="password"
            className="field mono flex-[1_1_320px] min-w-0"
            aria-label={t("discord.urlLabel")}
            placeholder="https://discord.com/api/webhooks/…"
            value={url}
            autoComplete="off"
            spellCheck={false}
            onChange={(e) => setUrl(e.target.value)}
          />
          <button type="submit" className="btn btn-primary" disabled={!url.trim() || busy !== null}>
            {busy === "save" ? t("discord.saving") : status?.configured ? t("discord.replace") : t("common.save")}
          </button>
        </form>

        <Heading id="discord-events">{t("discord.events")}</Heading>
        {EVENTS.map(([key, title, hint]) => (
          <Row key={key} title={t(title)} hint={t(hint)}>
            <Switch label={t(title)} checked={events[key]} onChange={(on) => setEvents({ [key]: on })} />
          </Row>
        ))}
        <Row title={t("discord.statusSummary")} hint={t("discord.statusSummaryHint")} last>
          <NumberField
            unit={t("discord.minutesUnit")}
            short={t("discord.min")}
            min={0}
            max={1440}
            value={events.statusEveryMin}
            onCommit={(statusEveryMin) => setEvents({ statusEveryMin })}
          />
        </Row>
      </div>
    </main>
  );
}
