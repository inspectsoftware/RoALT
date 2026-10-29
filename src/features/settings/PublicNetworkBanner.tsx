import type { Settings } from "../../api";
import { useNetwork } from "../../data";
import { Icon } from "../../ui/Icon";
import { t } from "../../i18n";

/** Under the title bar while a Public network is connected (`network`): what RoALT refuses
 * there, or, when public networks are allowed, that this is a risk. */
export function PublicNetworkBanner({ settings }: { settings: Settings }) {
  if (!useNetwork().data?.public) return null;
  return (
    <div role="status" className="enter flex-none flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-2 bg-panel border-b border-line">
      <Icon name="lock" />
      <span className="flex-1 min-w-0">
        {settings.allowPublicNetworks ? (
          <span className="alert">{t("settings.publicBannerAllowed")}</span>
        ) : (
          t("settings.publicBannerBlocked")
        )}
      </span>
    </div>
  );
}
