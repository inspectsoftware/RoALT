import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, asError, type AccountInfo, type AccountPatch, type AccountSummary, type Group, type Presence, type VaultMode } from "../../api";
import { groupColor, qk, useAccountInfo, useUpdateAccount } from "../../data";
import { Switch } from "../../ui/controls";
import { Icon } from "../../ui/Icon";
import { friendly, toast, toastError } from "../../ui/Toast";
import { PasswordDialog } from "../vault/PasswordDialog";
import { accountAge, ago, Avatar, formatDate, PresenceStatus, Status, SWATCHES } from "./shared";

const REVEAL_SECONDS = 20;
const HEX = /^#[0-9a-f]{6}$/i;
const NO_ANSWER = "Roblox didn't answer";
const STATS = ["Robux", "Friends", "Followers", "Following", "Joined", "Age"];

const clip = (text: string, done: string) => writeText(text).then(() => toast(done), toastError);
const openLink = (url: string) => openUrl(url).catch(toastError);

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
  const qc = useQueryClient();
  const update = useUpdateAccount();
  const save = (patch: AccountPatch) => update.mutate({ id: a.id, patch });
  const info = useAccountInfo(a.id);
  const refresh = useMutation({
    mutationFn: () => api.accountInfo(a.id, true),
    onSuccess: (fresh) => {
      qc.setQueryData(qk.info(a.id), fresh);
      // account_info just cached the new avatar URL; the list reads it from there.
      qc.invalidateQueries({ queryKey: qk.headshots });
    },
  });
  const session = useMutation({
    mutationFn: () => api.accountOpenSession(a.id),
    onSuccess: () =>
      toast(
        a.status === "valid"
          ? "Opened a signed-in roblox.com window. Signing out there signs this account out of RoALT too."
          : "Opened roblox.com for this account. Sign in or finish the check there; RoALT keeps the new session when you close the window.",
      ),
  });
  const [alias, setAlias] = useState(a.alias);
  const [notes, setNotes] = useState(a.notes);
  const [hex, setHex] = useState(a.color ?? "");
  const [cookie, setCookie] = useState<string | null>(null);
  const [ask, setAsk] = useState<"reveal" | "copy" | null>(null);
  const [lastAsk, setLastAsk] = useState<"reveal" | "copy">("reveal");
  // Only info that belongs to this Roblox user (local ids can be reused after a removal).
  const d = info.data?.userId === a.userId ? info.data : undefined;
  const username = d?.username || a.username;
  const busy = refresh.isPending || info.isFetching;

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

  const copyId = () => clip(String(a.userId), "User ID copied");
  const copyName = () => clip(username, "Username copied");

  return (
    <div className="enter flex flex-col">
      <div className="flex gap-4 items-start p-5 border-b border-line">
        <Avatar account={a} size={72} url={d?.headshotUrl} />
        <div className="min-w-0 flex flex-col gap-1">
          <div className="text-lg font-semibold tracking-[-0.01em] truncate">{d?.displayName || a.displayName}</div>
          <div className="flex items-center gap-1.5 min-w-0">
            <span className="mono text-dim truncate selectable">@{username}</span>
            <CopyButton label="Copy username" onClick={copyName} />
          </div>
          <div className="flex items-center gap-1.5">
            <span className="mono text-xs text-dim selectable">ID {a.userId}</span>
            <CopyButton label="Copy user ID" onClick={copyId} />
          </div>
        </div>
      </div>

      <div className="flex flex-col gap-1 px-5 py-2.5 border-b border-line">
        <div className="flex items-center justify-between gap-2">
          <span className="mono text-[11px] text-dim">
            {d ? `Updated ${ago(d.fetchedAt)}` : info.isError ? "Not loaded" : "Loading…"}
          </span>
          <button className="btn btn-sm" onClick={() => refresh.mutate()} disabled={busy} title="Ask Roblox again">
            <Icon name="refresh" size={12} className={busy ? "spin" : ""} />
            Refresh
          </button>
        </div>
        {d && d.unavailable.length > 0 && (
          <div className="text-xs text-dim">Roblox didn't answer for: {d.unavailable.join(", ")}.</div>
        )}
      </div>

      {d ? (
        <Info d={d} />
      ) : info.isError ? (
        <div className="flex flex-col items-start gap-3 px-5 py-4 border-b border-line">
          <div role="alert" className="alert">{friendly(asError(info.error))}</div>
          <button className="btn btn-sm" onClick={() => info.refetch()} disabled={info.isFetching}>
            <Icon name="refresh" size={12} className={info.isFetching ? "spin" : ""} />
            Try again
          </button>
        </div>
      ) : (
        <InfoSkeleton />
      )}

      <Section>
        <div className="grid grid-cols-2 gap-2">
          <ActionButton icon="external" label="Open profile" onClick={() => openLink(`https://www.roblox.com/users/${a.userId}/profile`)} />
          <ActionButton
            icon="external"
            label="Open on roblox.com"
            title="Opens roblox.com signed in as this account, in its own window"
            disabled={session.isPending}
            onClick={() => session.mutate()}
          />
          <ActionButton icon="copy" label="Copy user ID" onClick={copyId} />
          <ActionButton icon="copy" label="Copy username" onClick={copyName} />
        </div>
      </Section>

      <Section>
        <div className="flex items-center justify-between gap-2">
          <Status status={a.status} checking={checking} />
          <span className="mono text-[11px] text-dim">checked {ago(a.statusCheckedAt)}</span>
        </div>
        <div className="mono text-[11px] text-dim">Added {formatDate(a.createdAt)}</div>
        <button className="btn" onClick={onValidate} disabled={checking}>
          <Icon name="refresh" size={14} className={checking ? "spin" : ""} />
          Validate
        </button>
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

function Info({ d }: { d: AccountInfo }) {
  const joined = d.created ? new Date(d.created) : null;
  const when = joined && !Number.isNaN(joined.getTime()) ? joined : null;
  const stats: Stat[] = [
    num("Robux", d.robux),
    num("Friends", d.friends),
    num("Followers", d.followers),
    num("Following", d.following),
    ["Joined", when?.toLocaleDateString(undefined, { month: "short", year: "numeric" }) ?? null, when?.toLocaleDateString(undefined, { dateStyle: "long" })],
    ["Age", d.created ? accountAge(d.created) : null],
  ];
  return (
    <>
      <div className="flex flex-wrap gap-1.5 px-5 py-3.5 border-b border-line">
        {badges(d).map((b) => (
          <span key={b.label} className="tag h-6 px-2" style={b.tone && TONE[b.tone]} title={b.title}>
            {b.label}
          </span>
        ))}
      </div>
      <div className="grid grid-cols-3 gap-px bg-line border-b border-line">
        {stats.map(([k, v, title]) => (
          <div key={k} className="min-w-0 bg-panel px-4 py-3">
            <div className="label">{k}</div>
            <div className="mono text-[15px] font-medium mt-0.5 truncate" title={v === null ? NO_ANSWER : title}>
              {v ?? "—"}
            </div>
          </div>
        ))}
      </div>
      <PresenceBlock p={d.presence} />
      {d.description.trim() && <About text={d.description} />}
    </>
  );
}

/** Label, value (null = no answer), tooltip. */
type Stat = [string, string | null, string?];

/** Big counts shrink to "1.2M" with the exact number as the tooltip. */
function num(k: string, n: number | null): Stat {
  if (n === null) return [k, null];
  if (n < 100_000) return [k, n.toLocaleString()];
  return [k, n.toLocaleString(undefined, { notation: "compact", maximumFractionDigits: 1 }), n.toLocaleString()];
}

type Badge = { label: string; tone?: keyof typeof TONE; title?: string };
const TONE: Record<"on" | "warn", CSSProperties> = {
  on: { color: "var(--text)", borderColor: "var(--line-strong)" },
  warn: { color: "var(--orange)", borderColor: "color-mix(in srgb, var(--orange) 55%, transparent)" },
};

function badges(d: AccountInfo): Badge[] {
  const out: Badge[] = [];
  if (d.isBanned) out.push({ label: "Banned", tone: "warn" });
  if (d.subscription === "plus") out.push({ label: "Plus", tone: "on" });
  if (d.subscription === "premium") out.push({ label: "Premium (legacy)", tone: "on" });
  const m = d.twoStepMethods;
  out.push(
    m === null
      ? { label: "2FA —", title: NO_ANSWER }
      : m.length
        ? { label: "2FA on", tone: "on", title: m.join(", ") }
        : { label: "2FA off" },
  );
  out.push(
    d.emailVerified === null
      ? { label: "Email —", title: NO_ANSWER }
      : d.emailVerified
        ? { label: "Email verified", tone: "on" }
        : { label: "Email not verified" },
  );
  if (d.verifiedBadge) out.push({ label: "Verified badge", tone: "on" });
  return out;
}

function PresenceBlock({ p }: { p: Presence | null }) {
  const job = p?.gameId ?? null;
  const place = p?.placeId ?? null;
  return (
    <div className="flex flex-col gap-2.5 px-5 py-4 border-b border-line">
      <span className="label">Presence</span>
      {!p ? (
        <span className="text-dim self-start" title={NO_ANSWER}>—</span>
      ) : p.kind !== "inGame" ? (
        <PresenceStatus presence={p} />
      ) : (
        <div className="flex flex-col gap-1.5">
          <div className="flex items-center gap-2 min-w-0">
            <span aria-hidden="true" className="w-2 h-2 flex-none bg-accent-text" />
            {p.lastLocation && <span className="sr-only">In game:</span>}
            <span className="font-semibold truncate" title={p.lastLocation ?? undefined}>{p.lastLocation ?? "In game"}</span>
          </div>
          {place !== null && (
            <div className="mono text-xs text-dim truncate selectable">
              place {place}
              {job && ` · job ${job.slice(0, 8)}…${job.slice(-4)}`}
            </div>
          )}
          {(place !== null || job) && (
            <div className="flex gap-2 mt-1">
              {place !== null && (
                <button className="btn flex-1 min-w-0" onClick={() => openLink(`https://www.roblox.com/games/${place}`)}>
                  <span className="truncate">Open game page</span>
                  <Icon name="external" size={12} />
                </button>
              )}
              {job && (
                <button className="btn" onClick={() => clip(job, "Job ID copied")} aria-label="Copy job ID" title="Copy job ID">
                  <Icon name="copy" size={12} />
                  Job ID
                </button>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

/** Three lines, with "Show more" only when the text is actually cut. */
function About({ text }: { text: string }) {
  const p = useRef<HTMLParagraphElement>(null);
  const [cut, setCut] = useState(false);
  const [more, setMore] = useState(false);
  useLayoutEffect(() => {
    const el = p.current;
    if (el) setCut(el.scrollHeight > el.clientHeight + 1);
  }, [text]);
  return (
    <div className="flex flex-col gap-1.5 px-5 py-4 border-b border-line">
      <span className="label">About</span>
      <p ref={p} className={`whitespace-pre-line break-words selectable ${more ? "" : "line-clamp-3"}`}>{text}</p>
      {cut && (
        <button className="linkbtn self-start text-xs text-dim" aria-expanded={more} onClick={() => setMore(!more)}>
          {more ? "Show less" : "Show more"}
        </button>
      )}
    </div>
  );
}

function InfoSkeleton() {
  return (
    <div aria-busy="true" aria-label="Loading account info">
      <div className="flex gap-1.5 px-5 py-3.5 border-b border-line">
        {[56, 64, 96].map((w) => (
          <span key={w} className="skel h-6" style={{ width: w }} />
        ))}
      </div>
      <div className="grid grid-cols-3 gap-px bg-line border-b border-line">
        {STATS.map((k) => (
          <div key={k} className="bg-panel px-4 py-3">
            <div className="label">{k}</div>
            <div className="skel h-[18px] w-12 mt-1" />
          </div>
        ))}
      </div>
      <div className="flex flex-col gap-2.5 px-5 py-4 border-b border-line">
        <span className="label">Presence</span>
        <span className="skel h-[18px] w-36" />
      </div>
    </div>
  );
}

function CopyButton({ label, onClick }: { label: string; onClick: () => void }) {
  return (
    <button className="wc w-7 h-7" aria-label={label} title={label} onClick={onClick}>
      <Icon name="copy" size={12} />
    </button>
  );
}

function ActionButton({ icon, label, title, disabled, onClick }: {
  icon: "external" | "copy";
  label: string;
  title?: string;
  disabled?: boolean;
  onClick: () => void;
}) {
  return (
    <button className="btn min-w-0 px-2.5" title={title ?? label} disabled={disabled} onClick={onClick}>
      {icon === "copy" && <Icon name="copy" size={12} />}
      <span className="truncate">{label}</span>
      {icon === "external" && <Icon name="external" size={12} />}
    </button>
  );
}

function Section({ children }: { children: ReactNode }) {
  return <div className="flex flex-col gap-3 px-5 py-4 border-b border-line">{children}</div>;
}
