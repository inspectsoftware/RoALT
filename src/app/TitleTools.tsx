import { t } from "../i18n";
import { IconButton } from "../ui/Button";
import { IconSwap } from "../ui/motion";

/** The title bar's own keys: refresh (as F5), the layout, the theme, and the vault (a lock key with a
 * master password, else the encryption's state). */
export function TitleTools({ dark, onRefresh, onToggleTheme, sideTabs, onToggleTabs, onLock }: {
  dark: boolean;
  onRefresh?: () => void;
  onToggleTheme?: () => void;
  sideTabs?: boolean;
  onToggleTabs?: () => void;
  onLock?: () => void;
}) {
  return (
    <div className="title-tools">
      {onRefresh && <IconButton icon="refresh" label={t("ui.refresh")} shortcut="F5" onClick={onRefresh} />}
      {onToggleTabs && (
        <IconButton icon={sideTabs ? "layoutTop" : "layoutSide"} label={sideTabs ? t("ui.accountsTop") : t("ui.accountsSide")} onClick={onToggleTabs} />
      )}
      {onToggleTheme && (
        <button type="button" className="wc" aria-label={dark ? t("ui.lightTheme") : t("ui.darkTheme")} data-tip={dark ? t("ui.lightTheme") : t("ui.darkTheme")} onClick={onToggleTheme}>
          <IconSwap name={dark ? "sun" : "moon"} />
        </button>
      )}
      {onLock && <IconButton icon="lock" label={t("ui.lockVault")} shortcut="Ctrl+L" onClick={onLock} />}
    </div>
  );
}
