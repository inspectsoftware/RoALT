import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useMutation } from "@tanstack/react-query";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, type AccountSummary } from "../../api";
import { useUpdateAccount } from "../../data";
import { Icon, type IconName } from "../../ui/Icon";
import { toast, toastError } from "../../ui/Toast";
import { plural } from "./shared";

export type MenuAt = { account: AccountSummary; x: number; y: number };

/** Right-click menu of an account row. With several rows ticked and this one among them,
 * Launch, Validate and Remove apply to all of them. */
export function AccountMenu({ at, onClose, selection, canLaunch, onLaunch, onValidate, onRemove }: {
  at: MenuAt | null;
  onClose: () => void;
  /** Ticked account ids. */
  selection: number[];
  canLaunch: (id: number) => boolean;
  onLaunch: (ids: number[]) => void;
  onValidate: (ids: number[]) => void;
  onRemove: (ids: number[]) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: 0, top: 0 });
  const update = useUpdateAccount();
  const session = useMutation({ mutationFn: (id: number) => api.accountOpenSession(id) });

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el || !at) return;
    el.showPopover();
    // Keep it inside the window.
    const r = el.getBoundingClientRect();
    setPos({ left: Math.max(4, Math.min(at.x, innerWidth - r.width - 4)), top: Math.max(4, Math.min(at.y, innerHeight - r.height - 4)) });
    el.querySelector<HTMLElement>(".menu-item")?.focus();
  }, [at]);

  useEffect(() => {
    const el = ref.current;
    const toggled = (e: Event) => (e as ToggleEvent).newState === "closed" && onClose();
    el?.addEventListener("toggle", toggled);
    return () => el?.removeEventListener("toggle", toggled);
  }, [onClose]);

  const a = at?.account;
  const ids = a && selection.length > 1 && selection.includes(a.id) ? selection : a ? [a.id] : [];
  const many = ids.length > 1;
  const anyValid = ids.some(canLaunch);
  const run = (f: () => unknown) => () => {
    ref.current?.hidePopover();
    void f();
  };
  const clip = (text: string, done: string) => writeText(text).then(() => toast(done), toastError);

  const items: ([IconName, string, () => unknown, boolean?] | null)[] = a
    ? [
        ["play", many ? `Launch ${plural(ids.length, "account")}` : "Launch", () => onLaunch(ids), !anyValid],
        ["signin", "Open on roblox.com", () => session.mutateAsync(a.id).catch(toastError)],
        ["external", "Open profile", () => openUrl(`https://www.roblox.com/users/${a.userId}/profile`).catch(toastError)],
        null,
        ["copy", "Copy username", () => clip(a.username, "Username copied")],
        ["copy", "Copy user ID", () => clip(String(a.userId), "User ID copied")],
        null,
        ["star", a.favourite ? "Remove from favourites" : "Add to favourites", () => update.mutate({ id: a.id, patch: { favourite: !a.favourite } })],
        ["refresh", many ? `Validate ${plural(ids.length, "account")}` : "Validate", () => onValidate(ids)],
        ["trash", many ? `Remove ${plural(ids.length, "account")}…` : "Remove…", () => onRemove(ids)],
      ]
    : [];

  return (
    <div
      ref={ref}
      popover="auto"
      role="menu"
      aria-label={a ? `Actions for ${a.username}` : "Account actions"}
      className="menu"
      style={{ position: "fixed", positionArea: "none", inset: "auto", left: pos.left, top: pos.top, margin: 0, maxHeight: "none" }}
    >
      {items.map((it, i) =>
        it === null ? (
          <div key={i} role="separator" className="h-px my-1 bg-line" />
        ) : (
          <button
            key={it[1]}
            role="menuitem"
            className={`menu-item ${it[0] === "trash" ? "text-orange" : ""}`}
            disabled={it[3]}
            onClick={run(it[2])}
          >
            <Icon name={it[0]} size={12} fill={it[0] === "star" && a?.favourite} />
            {it[1]}
          </button>
        ),
      )}
    </div>
  );
}
