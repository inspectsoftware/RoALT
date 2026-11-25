import { useEffect, useId, useRef, useState } from "react";
import { motion } from "motion/react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl } from "@tauri-apps/plugin-opener";
import { t } from "../../i18n";
import { Button } from "../../ui/Button";
import { Dialog } from "../../ui/Dialog";
import { Icon } from "../../ui/Icon";
import { DUR, RISE, SPRING, staggerDelay } from "../../ui/motion";
import { toast, toastError } from "../../ui/Toast";
import { DISCORD_INVITE } from "../donate/links";
import { useOpenCount } from "../vault/useOpenCount";
import "./hallOfFame.css";

/** Contributors and beta testers who have helped build and test RoALT (Discord usernames). */
const CONTRIBUTORS: readonly string[] = [
  "foolish",
  "kurai",
  "itadevz",
  "dreamcatcher",
  "dxaee",
  "cookedasalways",
  "endles_gluttony",
];

/** Recognises the contributors and beta testers who helped shape RoALT. */
export function HallOfFameDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const opened = useOpenCount(open);
  const textId = useId();
  return (
    <Dialog open={open} onClose={onClose} title={t("hallOfFame.title")} width={460} describedBy={textId}>
      <HallOfFameBody key={opened} textId={textId} onClose={onClose} />
    </Dialog>
  );
}

function HallOfFameBody({ textId, onClose }: { textId: string; onClose: () => void }) {
  const [copied, setCopied] = useState<string | null>(null);
  const timer = useRef(0);
  useEffect(() => () => clearTimeout(timer.current), []);

  const copyName = (name: string) => {
    writeText(name).then(() => {
      setCopied(name);
      toast(t("hallOfFame.copied", { name }), "ok");
      clearTimeout(timer.current);
      timer.current = window.setTimeout(() => setCopied(null), 1600);
    }, toastError);
  };

  return (
    <>
      <div className="dlg-body flex flex-col gap-3">
        <p id={textId} className="text-fg-2">{t("hallOfFame.text")}</p>
        <div className="hof-grid" role="list" aria-label={t("hallOfFame.title")}>
          {CONTRIBUTORS.map((name, index) => {
            const isCopied = copied === name;
            return (
              <motion.button
                key={name}
                type="button"
                role="listitem"
                className="hof-card"
                data-copied={isCopied || undefined}
                data-tip={t("hallOfFame.copyHint")}
                aria-label={`${name} · ${t("hallOfFame.copyHint")}`}
                initial={{ opacity: 0, y: RISE.sm }}
                animate={{ opacity: 1, y: 0 }}
                transition={{ ...SPRING.smooth, delay: DUR.instant / 1000 + staggerDelay(index) }}
                onClick={() => copyName(name)}
              >
                <Icon name="discord" size={15} className="hof-icon" />
                <span className="hof-name">{name}</span>
                <Icon name={isCopied ? "check" : "copy"} size={13} className="hof-copy" />
              </motion.button>
            );
          })}
        </div>
      </div>
      <div className="dlg-foot items-center justify-between">
        <span className="text-[13px] text-fg-3">{t("hallOfFame.discordNote")}</span>
        <Button
          variant="ghost"
          size="md"
          iconEnd="external"
          onClick={() => {
            openUrl(DISCORD_INVITE).catch(toastError);
            onClose();
          }}
        >
          {t("app.joinDiscord")}
        </Button>
      </div>
    </>
  );
}
