import { useEffect, useState, type ReactNode } from "react";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, type AccountPatch, type AccountSummary, type Group, type VaultMode } from "../../api";
import { groupColor, useUpdateAccount } from "../../data";
import { Switch } from "../../ui/controls";
import { Icon } from "../../ui/Icon";
import { toast, toastError } from "../../ui/Toast";
import { PasswordDialog } from "../vault/PasswordDialog";
import { ago, Avatar, formatDate, Status, SWATCHES } from "./shared";

const REVEAL_SECONDS = 20;
const HEX = /^#[0-9a-f]{6}$/i;

/** The parent keys this by account id, so local edits reset when the selection changes. */
export function DetailPanel({ account: a, groups, vaultMode, clipboardSeconds, checking, onValidate, onRemove }: {
  account: AccountSummary;
  groups: Group[];
  vaultMode: VaultMode;
  clipboardSeconds: number;
  checking: boolean;
  onValidate: () => void;
  onRemove: () => void;
}) {
  const update = useUpdateAccount();
  const save = (patch: AccountPatch) => update.mutate({ id: a.id, patch });
  const [alias, setAlias] = useState(a.alias);
  const [notes, setNotes] = useState(a.notes);
  const [hex, setHex] = useState(a.color ?? "");
  const [cookie, setCookie] = useState<string | null>(null);
  const [ask, setAsk] = useState<"reveal" | "copy" | null>(null);
  const [lastAsk, setLastAsk] = useState<"reveal" | "copy">("reveal");

  // Revealed cookies hide themselves; unmounting (other account, lock) drops them too.
  useEffect(() => {
    if (!cookie) return;
    const t = setTimeout(() => setCookie(null), REVEAL_SECONDS * 1000);
    return () => clearTimeout(t);
  }, [cookie]);

  const reveal = async (password?: string) => setCookie(await api.accountRevealCookie(a.id, password));
  const copy = async (password?: string) => {
    await api.accountCopyCookie(a.id, password);
    toast(`Cookie copied. The clipboard clears in ${clipboardSeconds} s.`);
  };
  const start = (kind: "reveal" | "copy") => {
    if (vaultMode === "master") {
      setLastAsk(kind);
      setAsk(kind);
    } else {
      (kind === "reveal" ? reveal() : copy()).catch(toastError);
    }
  };

  const commitHex = () => {
    const v = hex.trim();
    if (v === (a.color ?? "")) return;
    if (v === "" || HEX.test(v)) save({ color: v.toLowerCase() });
    else setHex(a.color ?? "");
  };

  const toggleGroup = (gid: number) =>
    save({ groupIds: a.groupIds.includes(gid) ? a.groupIds.filter((x) => x !== gid) : [...a.groupIds, gid] });

  return (
    <div className="enter flex flex-col">
      <div className="flex gap-4 items-start p-5 border-b border-line">
        <Avatar account={a} size={64} />
        <div className="min-w-0 flex flex-col gap-1">
          <div className="text-lg font-semibold tracking-[-0.01em] truncate">{a.displayName}</div>
          <div className="mono text-dim truncate selectable">@{a.username}</div>
          <div className="flex items-center gap-1.5">
            <span className="mono text-xs text-dim selectable">ID {a.userId}</span>
            <button
              className="wc w-7 h-7"
              aria-label="Copy user ID"
              title="Copy user ID"
              onClick={() => writeText(String(a.userId)).then(() => toast("User ID copied"), toastError)}
            >
              <Icon name="copy" size={12} />
            </button>
          </div>
        </div>
      </div>

      <Section>
        <div className="flex items-center justify-between gap-2">
          <Status status={a.status} checking={checking} />
          <span className="mono text-[11px] text-dim">checked {ago(a.statusCheckedAt)}</span>
        </div>
        <div className="mono text-[11px] text-dim">Added {formatDate(a.createdAt)}</div>
        <div className="grid grid-cols-2 gap-2 pt-1">
          <button className="btn" onClick={onValidate} disabled={checking}>
            <Icon name="refresh" size={14} className={checking ? "spin" : ""} />
            Validate
          </button>
          <button
            className="btn"
            onClick={() => openUrl(`https://www.roblox.com/users/${a.userId}/profile`).catch(toastError)}
          >
            Open profile
            <Icon name="external" size={12} />
          </button>
        </div>
      </Section>

      <Section>
        <label className="flex flex-col gap-1.5">
          <span className="label">Alias</span>
          <input
            className="field"
            value={alias}
            placeholder={a.displayName}
            maxLength={64}
            onChange={(e) => setAlias(e.target.value)}
            onBlur={() => alias.trim() !== a.alias && save({ alias: alias.trim() })}
            onKeyDown={(e) => {
              if (e.key === "Enter") e.currentTarget.blur();
              if (e.key === "Escape" && alias !== a.alias) {
                e.stopPropagation();
                setAlias(a.alias);
              }
            }}
          />
        </label>
        <label className="flex flex-col gap-1.5">
          <span className="label">Notes</span>
          <textarea
            className="field"
            rows={3}
            value={notes}
            maxLength={2000}
            onChange={(e) => setNotes(e.target.value)}
            onBlur={() => notes !== a.notes && save({ notes })}
          />
        </label>
        <div className="flex flex-col gap-1.5">
          <span className="label" id="colour-label">Colour</span>
          <div className="flex flex-wrap items-center gap-1.5" role="group" aria-labelledby="colour-label">
            {SWATCHES.map((c) => (
              <button
                key={c}
                className="swatch"
                style={{ background: c }}
                aria-label={`Colour ${c}`}
                aria-pressed={a.color?.toLowerCase() === c.toLowerCase()}
                onClick={() => {
                  setHex(c.toLowerCase());
                  save({ color: c.toLowerCase() });
                }}
              />
            ))}
            <input
              className="field mono h-7 w-[86px] px-2 text-xs"
              aria-label="Custom colour, #rrggbb"
              placeholder="#rrggbb"
              value={hex}
              maxLength={7}
              spellCheck={false}
              aria-invalid={hex !== "" && !HEX.test(hex)}
              onChange={(e) => setHex(e.target.value)}
              onBlur={commitHex}
              onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
            />
          </div>
        </div>
        <div className="flex flex-col gap-1.5">
          <span className="label" id="groups-label">Groups</span>
          {groups.length === 0 ? (
            <span className="text-dim text-xs">No groups yet. Add one in the sidebar.</span>
          ) : (
            <div className="flex flex-wrap gap-1.5" role="group" aria-labelledby="groups-label">
              {groups.map((g) => (
                <button key={g.id} className="chip chip-sm" aria-pressed={a.groupIds.includes(g.id)} onClick={() => toggleGroup(g.id)}>
                  <span aria-hidden="true" className="w-1.5 h-1.5" style={{ background: groupColor(g) }} />
                  {g.name}
                </button>
              ))}
            </div>
          )}
        </div>
        <div className="flex items-center justify-between gap-2">
          <span>Favourite</span>
          <Switch label="Favourite" checked={a.favourite} onChange={(v) => save({ favourite: v })} />
        </div>
      </Section>

      <Section>
        <div className="flex items-center justify-between">
          <span className="label">Cookie</span>
          <span className="mono text-[11px] text-dim">
            {a.status === "invalid" ? "expired · sign in again" : "encrypted on this PC"}
          </span>
        </div>
        {cookie ? (
          <div className="enter flex flex-col border border-line-strong">
            <div className="mono text-[11px] leading-snug p-2.5 max-h-28 overflow-auto break-all selectable scroll">{cookie}</div>
            <div className="h-0.5 bg-line">
              <div className="h-full bg-accent-text drain" />
            </div>
          </div>
        ) : (
          <span className="mono h-[34px] flex items-center px-2.5 border border-line text-dim tracking-[0.1em] overflow-hidden">
            ••••••••••••••••••••
          </span>
        )}
        <div className="grid grid-cols-2 gap-2">
          <button className="btn" onClick={() => (cookie ? setCookie(null) : start("reveal"))}>
            <Icon name={cookie ? "eyeOff" : "lock"} size={14} />
            {cookie ? "Hide" : "Reveal"}
          </button>
          <button className="btn" onClick={() => start("copy")}>
            <Icon name="copy" size={14} />
            Copy
          </button>
        </div>
        <div className="text-xs text-dim">
          {cookie
            ? `Hides in ${REVEAL_SECONDS} s. Never share it: it signs in as this account.`
            : vaultMode === "master"
              ? "Reveal and copy ask for your master password first."
              : "Reveal and copy ask for Windows Hello first."}
        </div>
      </Section>

      <div className="p-5">
        <button className="btn btn-danger w-full" onClick={onRemove}>
          <Icon name="trash" size={14} />
          Remove account
        </button>
      </div>

      <PasswordDialog
        open={ask !== null}
        onClose={() => setAsk(null)}
        title={lastAsk === "reveal" ? "Reveal cookie" : "Copy cookie"}
        text={`Enter your master password to ${lastAsk} the cookie for @${a.username}.`}
        submitLabel={lastAsk === "reveal" ? "Reveal" : "Copy"}
        current
        onSubmit={({ current }) => (lastAsk === "reveal" ? reveal(current) : copy(current))}
      />
    </div>
  );
}

function Section({ children }: { children: ReactNode }) {
  return <div className="flex flex-col gap-3 px-5 py-4 border-b border-line">{children}</div>;
}
