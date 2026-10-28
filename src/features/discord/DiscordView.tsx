import { useState, type FormEvent } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type DiscordEvents, type Settings } from "../../api";
import { qk, useSetSettings } from "../../data";
import { NumberField, Switch } from "../../ui/controls";
import { toast, toastError } from "../../ui/Toast";
import { Heading, Row } from "../settings/SettingsView";

const EVENTS: [key: Exclude<keyof DiscordEvents, "statusEveryMin">, title: string, hint: string][] = [
  ["crash", "Crashes", "A client closed without RoALT closing it."],
  ["hang", "Stopped responding", "A client's window hasn't responded for 30 s."],
  ["disconnect", "Disconnects", "Roblox kicked a client, its server shut down or the connection was lost."],
  ["restart", "Rejoins and restarts", "Auto-rejoin or auto-restart launched an account again."],
  ["memRestart", "Memory restarts", "A client was restarted because it used too much memory."],
  ["breaker", "Gave up", "RoALT stopped relaunching an account after too many tries."],
  ["batchDone", "Batch finished", "How many accounts of a batch made it into the game."],
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
        toast("Webhook saved. Send a test to check it.");
      }, toastError)
      .finally(() => setBusy(null));
  };
  const remove = () => {
    setBusy("remove");
    api
      .discordSet(null)
      .then((st) => {
        qc.setQueryData(qk.discord, st);
        toast("Webhook removed. RoALT sends nothing to Discord now.");
      }, toastError)
      .finally(() => setBusy(null));
  };
  const test = () => {
    setBusy("test");
    api
      .discordTest()
      .then(() => toast("Test message sent."), toastError)
      .finally(() => setBusy(null));
  };

  return (
    <main className="scroll flex-1 min-w-0 px-10 pt-6 pb-12">
      <div className="enter max-w-[760px] flex flex-col">
        <h1 className="text-xl font-semibold tracking-[-0.01em] mb-2">Discord</h1>
        <p className="text-dim max-w-[620px]">
          Get told in a Discord channel when a client crashes, gets disconnected or is relaunched. RoALT posts to a webhook
          you create in the channel's settings (Integrations, Webhooks). Discord is the only place besides Roblox RoALT
          sends anything to, and only once a webhook is set. Messages name accounts by display name and username; never
          a cookie, link or log.
        </p>

        <Heading id="discord-webhook">Webhook</Heading>
        <Row
          title="Webhook"
          hint={
            status?.configured && !status.webhook
              ? "Set, but the vault is locked. Unlock it to see or change the webhook."
              : status?.configured
              ? `Set (id ${status.webhook ?? "unknown"}). The URL is encrypted in the vault; messages go out only while it is unlocked.`
              : "Not set. Paste the webhook URL Discord shows (Copy Webhook URL)."
          }
        >
          {status?.configured && (
            <div className="flex gap-2">
              <button className="btn" disabled={busy !== null} onClick={test}>{busy === "test" ? "Sending…" : "Send test"}</button>
              <button className="btn" disabled={busy !== null} onClick={remove}>Remove</button>
            </div>
          )}
        </Row>
        <form onSubmit={save} className="flex flex-wrap items-center gap-2 py-4 border-b border-line">
          <input
            type="password"
            className="field mono flex-[1_1_320px] min-w-0"
            aria-label="Discord webhook URL"
            placeholder="https://discord.com/api/webhooks/…"
            value={url}
            autoComplete="off"
            spellCheck={false}
            onChange={(e) => setUrl(e.target.value)}
          />
          <button type="submit" className="btn btn-primary" disabled={!url.trim() || busy !== null}>
            {busy === "save" ? "Saving…" : status?.configured ? "Replace" : "Save"}
          </button>
        </form>

        <Heading id="discord-events">Events</Heading>
        {EVENTS.map(([key, title, hint]) => (
          <Row key={key} title={title} hint={hint}>
            <Switch label={title} checked={events[key]} onChange={(on) => setEvents({ [key]: on })} />
          </Row>
        ))}
        <Row title="Status summary" hint="Every so often while clients run: how many, their RAM and CPU. 0 turns it off." last>
          <NumberField
            unit="minutes, 0 for off"
            short="min"
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
