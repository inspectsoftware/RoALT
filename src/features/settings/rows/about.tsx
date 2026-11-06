import { SettingRow } from "../kit/SettingRow";

// AltDock has no update source yet, no online counter and no community links of its own: About
// only names the version (RoALT's update check, counter, releases, Discord and Donate are gone).

/** AltDock's own version: the one place it shows. */
export function VersionRow() {
  const version = import.meta.env.APP_VERSION;
  return <SettingRow id="version" name={`AltDock ${version}`} title={<span className="set-label">AltDock <span className="mono">{version}</span></span>} />;
}
