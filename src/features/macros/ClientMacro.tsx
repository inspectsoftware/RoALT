// A client's macro, in its card's menu: run or stop it, and which macro its account runs.
import { api, type Instance } from "../../api";
import { qkA, queryClient, useMacroAssignments, useMacroRuns, useMacros } from "../../data";
import { isRtl, t } from "../../i18n";
import { Icon } from "../../ui/Icon";
import { MenuCheckItem, MenuItem, MenuRadioItem, MenuSeparator } from "../../ui/Popover";
import { toastError } from "../../ui/Toast";

/** The macro this client runs, if any. */
export const useClientMacroRun = (instanceId: string) => useMacroRuns().data?.find((r) => r.instanceId === instanceId);

function useAssigned(accountId: number) {
  const assigned = useMacroAssignments().data?.find((a) => a.accountId === accountId);
  const macro = useMacros().data?.find((m) => m.id === assigned?.macroId);
  return { assigned, macro };
}

/** Stop the macro it runs, or run its account's; then the account's macro, which opens `onList`. */
export function MacroMenuItems({ inst, onList }: { inst: Instance; onList: () => void }) {
  const run = useClientMacroRun(inst.id);
  const { macro } = useAssigned(inst.accountId);
  const start = (id: number) => api.macroRun(id, [inst.id]).then((runs) => queryClient.setQueryData(qkA.macroRuns, runs), toastError);
  return (
    <>
      {run ? (
        <MenuItem icon="stop" onSelect={() => api.macroStop(inst.id).catch(toastError)}>{t("dashboard.stopMacroName", { name: run.name })}</MenuItem>
      ) : (
        macro && (
          <MenuItem icon="play" disabled={inst.state !== "inGame" || !macro.trusted} onSelect={() => void start(macro.id)}>
            {t("dashboard.runMacroName", { name: macro.name })}
          </MenuItem>
        )
      )}
      <button type="button" role="menuitem" aria-haspopup="menu" className="menu-item" onClick={onList}>
        <Icon name="key" />
        <span className="flex-1 min-w-0 truncate">{macro ? t("dashboard.macroIs", { name: macro.name }) : t("dashboard.macroPick")}</span>
        <Icon name="chevron" size={14} className={isRtl() ? "rotate-90" : "-rotate-90"} />
      </button>
    </>
  );
}

/** The account's macro: none or one of the trusted ones, and whether it starts by itself once a
 * client of the account reaches a game. */
export function MacroPickList({ accountId, onBack }: { accountId: number; onBack: () => void }) {
  const macros = (useMacros().data ?? []).filter((m) => m.trusted);
  const { assigned } = useAssigned(accountId);
  const assign = (macroId: number | null, autorun: boolean) =>
    api.macroAssign(accountId, macroId, autorun).then((list) => queryClient.setQueryData(qkA.macroAssignments, list), toastError);
  return (
    <>
      <MenuItem icon="back" closes={false} onSelect={onBack}>{t("dashboard.back")}</MenuItem>
      <MenuSeparator />
      <MenuRadioItem checked={!assigned} onSelect={() => void assign(null, false)}>{t("dashboard.noMacro")}</MenuRadioItem>
      {macros.map((m) => (
        <MenuRadioItem key={m.id} checked={assigned?.macroId === m.id} onSelect={() => void assign(m.id, assigned?.autorun ?? true)}>
          {m.name}
        </MenuRadioItem>
      ))}
      <MenuSeparator />
      <MenuCheckItem checked={assigned?.autorun ?? false} disabled={!assigned} onChange={(on) => assigned && void assign(assigned.macroId, on)}>
        {t("dashboard.macroAutorun")}
      </MenuCheckItem>
    </>
  );
}
