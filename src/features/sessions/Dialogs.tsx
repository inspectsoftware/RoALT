import { useEffect, useState } from "react";
import { sessionsApi, type AccountSummary, type SessionConfig, type SessionMode } from "../../api";
import { useAccounts } from "../../data";
import { t } from "../../i18n";
import { Button } from "../../ui/Button";
import { Segmented } from "../../ui/controls";
import { Dialog, DialogBody, DialogFooter } from "../../ui/Dialog";
import { Checkbox, Field, Input } from "../../ui/Field";
import { toast } from "../../ui/Toast";
import { accountName, Avatar } from "../accounts/shared";
import { nextSlot, sk, useAct, useLocalMacros } from "./queries";

const NAME = /^[A-Za-z0-9 _-]{1,32}$/;
const SIZE = { w: [800, 3840], h: [600, 2160] } as const;

/** Add a session, or edit one (`editing`; its name stays, it is the key). Windows user, address and
 * port come from the next free slot and don't change afterwards. */
export function SessionDialog({ open, onClose, editing, sessions }: {
  open: boolean;
  onClose: () => void;
  editing: SessionConfig | null;
  sessions: SessionConfig[];
}) {
  const accounts = useAccounts().data ?? [];
  const { busy, run } = useAct();
  const [name, setName] = useState("");
  const [mode, setMode] = useState<SessionMode>("shared");
  const [ids, setIds] = useState<number[]>([]);
  const [width, setWidth] = useState("1280");
  const [height, setHeight] = useState("720");
  const [tried, setTried] = useState(false);
  useEffect(() => {
    if (!open) return;
    setName(editing?.name ?? "");
    setMode(editing?.mode ?? "shared");
    setIds(editing?.accountIds ?? []);
    setWidth(String(editing?.width ?? 1280));
    setHeight(String(editing?.height ?? 720));
    setTried(false);
  }, [open, editing]);

  const slot = editing ?? nextSlot(sessions);
  const others = sessions.filter((s) => s.name !== editing?.name);
  // Accounts in another session: one account runs in one place.
  const elsewhere = new Map(others.flatMap((s) => s.accountIds.map((id) => [id, s.name] as const)));
  const nameError = !name.trim() ? t("sessions.dialog.nameNeeded")
    : !NAME.test(name.trim()) ? t("sessions.dialog.nameChars")
    : others.some((s) => s.name.toLowerCase() === name.trim().toLowerCase()) ? t("sessions.dialog.nameTaken")
    : null;
  const inRange = (v: string, [lo, hi]: readonly [number, number]) => Number.isInteger(Number(v)) && Number(v) >= lo && Number(v) <= hi;
  const sizeError = inRange(width, SIZE.w) && inRange(height, SIZE.h) ? null : t("sessions.dialog.sizeRange", { w0: SIZE.w[0], w1: SIZE.w[1], h0: SIZE.h[0], h1: SIZE.h[1] });
  const dedicatedError = mode === "dedicated" && ids.length > 1 ? t("sessions.dialog.dedicatedOne") : null;

  const toggle = (a: AccountSummary, on: boolean) =>
    setIds((cur) => (mode === "dedicated" ? (on ? [a.id] : []) : on ? [...cur, a.id] : cur.filter((x) => x !== a.id)));

  const save = () => {
    setTried(true);
    if (nameError || sizeError || dedicatedError || busy) return;
    const config: SessionConfig = { ...slot, name: name.trim(), mode, accountIds: ids, width: Number(width), height: Number(height) };
    void run("save", () => sessionsApi.sessionSave(config), [sk.health]).then((ok) => {
      if (!ok) return;
      toast(editing ? t("sessions.dialog.saved") : t("sessions.dialog.added", { name: config.name }));
      onClose();
    });
  };

  return (
    <Dialog open={open} onClose={onClose} title={editing ? t("sessions.dialog.editTitle", { name: editing.name }) : t("sessions.dialog.addTitle")} width={520}>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          save();
        }}
      >
        <DialogBody className="flex flex-col gap-5">
          <Field label={t("sessions.dialog.name")} error={tried ? nameError : null} hint={editing ? t("sessions.dialog.nameFixed") : undefined}>
            {({ id, describedBy, invalid }) => (
              <Input id={id} aria-describedby={describedBy} invalid={invalid} value={name} disabled={!!editing} maxLength={32} placeholder={t("sessions.dialog.namePlaceholder")} onChange={(e) => setName(e.target.value)} />
            )}
          </Field>

          <div className="flex flex-col gap-1.5">
            <span className="label">{t("sessions.dialog.mode")}</span>
            <Segmented
              label={t("sessions.dialog.mode")}
              value={mode}
              onChange={(m) => {
                setMode(m);
                if (m === "dedicated") setIds((cur) => cur.slice(0, 1));
              }}
              options={[
                { value: "shared", label: t("sessions.mode.shared") },
                { value: "dedicated", label: t("sessions.mode.dedicated") },
              ]}
              className="self-start"
            />
            <p className="text-[13px] text-fg-3">{mode === "shared" ? t("sessions.mode.sharedHint") : t("sessions.mode.dedicatedHint")}</p>
          </div>

          <div className="flex flex-col gap-1.5">
            <span className="label" id="ses-accounts-label">
              {t("sessions.dialog.accounts")} <span className="text-fg-3 font-normal tabular-nums">{ids.length}</span>
            </span>
            <div role="group" aria-labelledby="ses-accounts-label" className="ses-pick scroll">
              {accounts.length === 0 && <p className="text-fg-3 text-[13px] p-2">{t("sessions.dialog.noAccounts")}</p>}
              {accounts.map((a) => (
                <div key={a.id} className="ses-pick-row">
                  <Checkbox
                    checked={ids.includes(a.id)}
                    onChange={(on) => toggle(a, on)}
                    label={
                      <span className="flex items-center gap-2 min-w-0">
                        <Avatar account={a} size={22} />
                        <span className="truncate">{accountName(a)}</span>
                        <span className="text-fg-3 truncate">@{a.username}</span>
                      </span>
                    }
                    className="min-w-0 flex-1"
                  />
                  {elsewhere.has(a.id) && <span className="tag flex-none">{elsewhere.get(a.id)}</span>}
                </div>
              ))}
            </div>
            {tried && dedicatedError ? <p role="alert" className="text-[13px] text-attention">{dedicatedError}</p> : <p className="text-[13px] text-fg-3">{t("sessions.dialog.accountsHint")}</p>}
          </div>

          <div className="flex flex-col gap-1.5">
            <span className="label" id="ses-size-label">{t("sessions.dialog.size")}</span>
            <div role="group" aria-labelledby="ses-size-label" className="flex items-center gap-2">
              <Input className="w-24" mono type="number" inputMode="numeric" aria-label={t("sessions.dialog.width")} min={SIZE.w[0]} max={SIZE.w[1]} value={width} invalid={tried && !!sizeError} onChange={(e) => setWidth(e.target.value)} />
              <span className="text-fg-3" aria-hidden="true">×</span>
              <Input className="w-24" mono type="number" inputMode="numeric" aria-label={t("sessions.dialog.height")} min={SIZE.h[0]} max={SIZE.h[1]} value={height} invalid={tried && !!sizeError} onChange={(e) => setHeight(e.target.value)} />
              <span className="text-fg-3">{t("sessions.dialog.px")}</span>
            </div>
            {tried && sizeError ? <p role="alert" className="text-[13px] text-attention">{sizeError}</p> : <p className="text-[13px] text-fg-3">{t("sessions.dialog.sizeHint")}</p>}
          </div>

          <p className="meta mono">{t("sessions.dialog.slot", { user: slot.windowsUser, address: slot.address, port: slot.port })}</p>
        </DialogBody>
        <DialogFooter>
          <Button size="md" onClick={onClose}>{t("common.cancel")}</Button>
          <Button size="md" type="submit" variant="primary" busy={busy === "save"}>
            {editing ? t("common.save") : t("sessions.add")}
          </Button>
        </DialogFooter>
      </form>
    </Dialog>
  );
}

/** Removing a session closes its clients; the alt Windows user (and its profile) only goes when asked. */
export function RemoveDialog({ session, onClose }: { session: SessionConfig | null; onClose: () => void }) {
  const { busy, run } = useAct();
  const [deleteUser, setDeleteUser] = useState(false);
  useEffect(() => {
    if (session) setDeleteUser(false);
  }, [session]);
  // Closed, the dialog keeps showing what it showed while it fades.
  const s = session;
  const remove = () => {
    if (!s || busy) return;
    void run("remove", () => sessionsApi.sessionRemove(s.name, deleteUser), [sk.health]).then((ok) => {
      if (!ok) return;
      toast(t("sessions.remove.done", { name: s.name }));
      onClose();
    });
  };
  return (
    <Dialog open={!!session} onClose={onClose} title={t("sessions.remove.title", { name: s?.name ?? "" })} width={440}>
      <DialogBody className="flex flex-col gap-4">
        <p className="text-fg-2">{t("sessions.remove.text")}</p>
        <Checkbox checked={deleteUser} onChange={setDeleteUser} label={t("sessions.remove.deleteUser", { user: s?.windowsUser ?? "" })} />
        {deleteUser && <p className="alert text-[13px]">{t("sessions.remove.deleteUserHint")}</p>}
      </DialogBody>
      <DialogFooter>
        <Button size="md" onClick={onClose} data-autofocus>{t("common.cancel")}</Button>
        <Button size="md" variant="danger" busy={busy === "remove"} onClick={remove}>{t("common.remove")}</Button>
      </DialogFooter>
    </Dialog>
  );
}

/** Which of the main app's macros go to the session (all, unless unticked). */
export function PushMacrosDialog({ session, onClose }: { session: string | null; onClose: () => void }) {
  const macros = useLocalMacros(!!session);
  const { busy, run } = useAct();
  const [off, setOff] = useState<Set<number>>(new Set());
  useEffect(() => {
    if (session) setOff(new Set());
  }, [session]);
  const list = macros.data ?? [];
  const picked = list.filter((m) => !off.has(m.id)).map((m) => m.id);
  const push = () => {
    if (!session || !picked.length || busy) return;
    void run("push", () => sessionsApi.sessionPushMacros(session, picked), [sk.macros(session)]).then((ok) => {
      if (!ok) return;
      toast(t("sessions.push.macrosDone", { count: picked.length, name: session }));
      onClose();
    });
  };
  return (
    <Dialog open={!!session} onClose={onClose} title={t("sessions.push.macrosTitle", { name: session ?? "" })} width={420}>
      <DialogBody className="flex flex-col gap-3">
        <p className="text-fg-2">{t("sessions.push.macrosText")}</p>
        {macros.isPending ? (
          <p className="text-fg-3">{t("common.loading")}</p>
        ) : list.length === 0 ? (
          <p className="text-fg-3">{t("sessions.push.noMacros")}</p>
        ) : (
          <div className="ses-pick scroll">
            {list.map((m) => (
              <div key={m.id} className="ses-pick-row">
                <Checkbox
                  checked={!off.has(m.id)}
                  label={m.name}
                  onChange={(on) =>
                    setOff((cur) => {
                      const next = new Set(cur);
                      if (on) next.delete(m.id);
                      else next.add(m.id);
                      return next;
                    })
                  }
                />
              </div>
            ))}
          </div>
        )}
      </DialogBody>
      <DialogFooter>
        <Button size="md" onClick={onClose}>{t("common.cancel")}</Button>
        <Button size="md" variant="primary" icon="upload" busy={busy === "push"} disabled={!picked.length} onClick={push}>
          {t("sessions.push.send", { count: picked.length })}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}
