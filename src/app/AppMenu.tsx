import { useState } from "react";
import type { VaultMode } from "../api";
import { t } from "../i18n";
import { Icon, Logo } from "../ui/Icon";
import { MenuItem, MenuSeparator, Popover, useAnchor } from "../ui/Popover";
import { KillAllButton } from "./KillAll";
import { requestNavigation } from "./navigation";

/** The logo and name, as the app's own menu: the vault's state and the keys that used to sit in the
 * sidebar's foot. While locked, only what needs no vault. (RoALT's online count, Discord button
 * and Donate are gone in AltDock.) */
export function AppMenu({ unlocked, vaultMode, onRefresh }: {
  unlocked: boolean;
  vaultMode?: VaultMode;
  onRefresh: () => void;
}) {
  const anchor = useAnchor();
  const [open, setOpen] = useState(false);
  const id = "app-menu";
  return (
    <>
      <button type="button" className="app-menu-btn" popoverTarget={id} style={anchor.style} aria-label="AltDock" aria-haspopup="menu" aria-expanded={open}>
        <Logo />
        <span className="wordmark">AltDock</span>
        <Icon name="chevron" size={12} className="app-menu-chevron" />
      </button>
      <Popover id={id} anchor={anchor.name} placement="below-start" role="menu" label="AltDock" onToggle={setOpen} className="w-[260px]">
        <div className="flex items-center gap-2 px-2.5 pt-1.5 pb-2 text-[13px] text-fg-3" title={!unlocked ? t("vault.title") : vaultMode === "master" ? t("app.vaultUnlockedMaster") : t("app.vaultUnlockedDpapi")}>
          <Icon name="lock" size={14} />
          {t("app.encrypted")}
        </div>
        <MenuSeparator />
        {unlocked && (
          <MenuItem icon="refresh" kbd="F5" onSelect={onRefresh}>
            {t("ui.refresh")}
          </MenuItem>
        )}
        {unlocked && (
          <MenuItem icon="keyboard" onSelect={() => requestNavigation("settings", "shortcuts")}>
            {t("settings.section.shortcuts")}
          </MenuItem>
        )}
        {unlocked && (
          <>
            <MenuSeparator />
            <div className="p-1" role="none">
              <KillAllButton role="menuitem" />
            </div>
          </>
        )}
      </Popover>
    </>
  );
}
