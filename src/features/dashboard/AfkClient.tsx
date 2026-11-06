// AltDock (Stream A): a client's own anti-AFK — how often and what it presses — kept for its account.
import { api, type AfkAction, type AfkClientConfig } from "../../api";
import { qkA, queryClient, useAfkClient } from "../../data";
import { t } from "../../i18n";
import { NumberField } from "../../ui/controls";
import { Select } from "../../ui/Field";
import { toastError } from "../../ui/Toast";

/** The keys the backend takes for a Key action (`instances::afk::KEYS`), as it spells them. */
export const AFK_KEYS: readonly string[] = [
  ..."ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789",
  ...Array.from({ length: 24 }, (_, n) => `F${n + 1}`),
  "Space", "Tab", "Up", "Down", "Left", "Right",
];

type Kind = "f13" | "space" | "jump" | "jumpTurn" | "key";
const kindOf = (a: AfkAction): Kind => (typeof a === "string" ? a : "key");

/** Interval and action of the account's anti-AFK; its on/off is the client's anti-AFK switch. */
export function AfkClientControls({ accountId }: { accountId: number }) {
  const cfg = useAfkClient(accountId).data;
  if (!cfg) return null;
  const set = (patch: Partial<AfkClientConfig>) =>
    api.antiAfkClientSet(accountId, { ...cfg, ...patch }).then((c) => queryClient.setQueryData(qkA.afkClient(accountId), c), toastError);
  const kind = kindOf(cfg.action);
  const kinds: { value: Kind; label: string }[] = [
    { value: "f13", label: t("afk.actionF13") },
    { value: "space", label: t("afk.actionSpace") },
    { value: "jump", label: t("afk.actionJump") },
    { value: "jumpTurn", label: t("afk.actionJumpTurn") },
    { value: "key", label: t("afk.actionKey") },
  ];
  return (
    <div className="flex flex-wrap items-center gap-x-4 gap-y-2" aria-disabled={!cfg.enabled || undefined}>
      <NumberField
        label={t("afk.every")}
        unit={t("afk.seconds")}
        short={t("afk.secondsShort")}
        min={60}
        max={1140}
        step={10}
        value={cfg.intervalSec}
        disabled={!cfg.enabled}
        onCommit={(intervalSec) => void set({ intervalSec })}
      />
      <Select aria-label={t("afk.action")} value={kind} options={kinds} disabled={!cfg.enabled} onChange={(k) => void set({ action: k === "key" ? { key: "W" } : k })} />
      {typeof cfg.action !== "string" && (
        <Select
          aria-label={t("afk.key")}
          value={cfg.action.key}
          options={AFK_KEYS.map((k) => ({ value: k, label: k }))}
          disabled={!cfg.enabled}
          onChange={(key) => void set({ action: { key } })}
        />
      )}
    </div>
  );
}
