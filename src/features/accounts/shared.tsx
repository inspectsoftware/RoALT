import { useState, type CSSProperties } from "react";
import type { AccountStatus, AccountSummary, Presence, PresenceKind } from "../../api";
import { useHeadshot } from "../../data";

/** Colours offered for accounts and groups (from the design canvas). */
export const SWATCHES = ["#C6F432", "#7AB0FF", "#5FE0C8", "#FFA05C", "#B49CFF", "#FF8FC7", "#ECEEF1", "#5E6672"];

export const accountName = (a: AccountSummary) => a.alias || a.displayName || a.username;

export function initials(a: AccountSummary) {
  return accountName(a).replace(/[^\p{L}\p{N}]/gu, "").slice(0, 2).toUpperCase() || "?";
}

/** Accounts without a colour get a stable one from their user id. */
export const avatarColor = (a: AccountSummary) => a.color ?? SWATCHES[a.userId % 6];

/** Dark ink on light colours, white on dark ones (custom #rrggbb can be anything). */
function inkOn(hex: string) {
  const n = parseInt(hex.slice(1), 16);
  const lum = 0.2126 * ((n >> 16) & 255) + 0.7152 * ((n >> 8) & 255) + 0.0722 * (n & 255);
  return lum > 110 ? "#0A0B0D" : "#FFFFFF";
}

/** The Roblox headshot over the account colour; the initials show until it loads, or if it fails. */
export function Avatar({ account, size, url }: { account: AccountSummary; size: number; url?: string | null }) {
  const src = useHeadshot(account.id) ?? url ?? null;
  const [loaded, setLoaded] = useState<string | null>(null);
  const [broken, setBroken] = useState<string | null>(null);
  const bg = avatarColor(account);
  const show = src !== null && src !== broken;
  const done = show && loaded === src;
  return (
    <span
      aria-hidden="true"
      className="relative flex-none grid place-content-center font-bold overflow-hidden"
      style={{ width: size, height: size, background: bg, color: inkOn(bg), fontSize: Math.round(size * 0.36) }}
    >
      <span className={done ? "opacity-0" : undefined}>{initials(account)}</span>
      {show && (
        <img
          key={src}
          src={src}
          alt=""
          loading="lazy"
          decoding="async"
          className={`absolute inset-0 w-full h-full object-cover ${done ? "opacity-100" : "opacity-0"}`}
          onLoad={() => setLoaded(src)}
          onError={() => setBroken(src)}
        />
      )}
    </span>
  );
}

/** Status is told apart by shape and label, never by colour alone. */
type Look = { label: string; color: string; mark: CSSProperties };
const hollow: CSSProperties = { border: "1px solid var(--grey)" };

const LOOK: Record<AccountStatus, Look> = {
  valid: { label: "Valid", color: "var(--text)", mark: { background: "var(--accent-text)" } },
  invalid: { label: "Cookie expired", color: "var(--orange)", mark: { background: "var(--orange)", transform: "rotate(45deg) scale(.85)" } },
  challenged: {
    label: "Needs a check",
    color: "var(--blue)",
    mark: { background: "var(--blue)", clipPath: "polygon(50% 0, 100% 100%, 0 100%)" },
  },
  unknown: { label: "Not checked", color: "var(--dim)", mark: hollow },
};

const PRESENCE: Record<PresenceKind, Look> = {
  inGame: { label: "In game", color: "var(--text)", mark: { background: "var(--accent-text)" } },
  online: { label: "Online", color: "var(--text)", mark: { background: "var(--blue)" } },
  studio: { label: "In Studio", color: "var(--text)", mark: { border: "2px solid var(--blue)" } },
  offline: { label: "Offline", color: "var(--dim)", mark: hollow },
  unknown: { label: "Unknown", color: "var(--dim)", mark: hollow },
};

function Mark({ look, label, detail }: { look: Look; label: string; detail?: string }) {
  return (
    <span className="inline-flex items-center gap-2 min-w-0 max-w-full" style={{ color: look.color }}>
      <span aria-hidden="true" className="w-2 h-2 flex-none" style={look.mark} />
      <span className="truncate" title={label}>{label}</span>
      {detail && <span className="mono text-xs text-dim truncate">{detail}</span>}
    </span>
  );
}

/** "In game · Sky Islands" where Roblox names the game. */
export function PresenceStatus({ presence: p }: { presence: Presence }) {
  const label = p.kind === "inGame" && p.lastLocation ? `In game · ${p.lastLocation}` : PRESENCE[p.kind].label;
  return <Mark look={PRESENCE[p.kind]} label={label} />;
}

/** Cookie status; a valid account shows its presence instead once that is known. */
export function Status({ status, presence, checking, detail }: {
  status: AccountStatus;
  presence?: Presence;
  checking?: boolean;
  detail?: string;
}) {
  if (checking) {
    return (
      <span className="inline-flex items-center gap-2 text-dim">
        <span aria-hidden="true" className="w-2 h-2 flex-none bg-dim pulse" />
        Checking…
      </span>
    );
  }
  if (status === "valid" && presence && presence.kind !== "unknown") return <PresenceStatus presence={presence} />;
  return <Mark look={LOOK[status]} label={LOOK[status].label} detail={detail} />;
}

export function ago(ms: number | null, now = Date.now()) {
  if (ms === null) return "never";
  const s = Math.max(0, (now - ms) / 1000);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return `${Math.floor(s / 86400)} d ago`;
}

export const formatDate = (ms: number) =>
  new Date(ms).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });

/** "7y 7m" since an ISO date; null when the date doesn't parse. */
export function accountAge(created: string, now = new Date()) {
  const c = new Date(created);
  if (Number.isNaN(c.getTime())) return null;
  const m = Math.max(
    0,
    (now.getFullYear() - c.getFullYear()) * 12 + now.getMonth() - c.getMonth() - (now.getDate() < c.getDate() ? 1 : 0),
  );
  return `${Math.floor(m / 12)}y ${m % 12}m`;
}

export const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
