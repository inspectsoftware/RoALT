import type { CSSProperties } from "react";
import type { AccountStatus, AccountSummary } from "../../api";

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

export function Avatar({ account, size }: { account: AccountSummary; size: number }) {
  const bg = avatarColor(account);
  return (
    <span
      aria-hidden="true"
      className="flex-none grid place-content-center font-bold"
      style={{ width: size, height: size, background: bg, color: inkOn(bg), fontSize: Math.round(size * 0.36) }}
    >
      {initials(account)}
    </span>
  );
}

/** Status is told apart by shape and label, never by colour alone. */
const LOOK: Record<AccountStatus, { label: string; color: string; mark: CSSProperties }> = {
  valid: { label: "Valid", color: "var(--text)", mark: { background: "var(--accent-text)" } },
  invalid: { label: "Expired", color: "var(--orange)", mark: { background: "var(--orange)", transform: "rotate(45deg) scale(.85)" } },
  challenged: {
    label: "Challenged",
    color: "var(--blue)",
    mark: { background: "var(--blue)", clipPath: "polygon(50% 0, 100% 100%, 0 100%)" },
  },
  unknown: { label: "Unknown", color: "var(--dim)", mark: { border: "1px solid var(--grey)" } },
};

export const statusLabel = (s: AccountStatus) => LOOK[s].label;

export function Status({ status, checking, detail }: { status: AccountStatus; checking?: boolean; detail?: string }) {
  if (checking) {
    return (
      <span className="inline-flex items-center gap-2 text-dim">
        <span aria-hidden="true" className="w-2 h-2 flex-none bg-dim pulse" />
        Checking…
      </span>
    );
  }
  const l = LOOK[status];
  return (
    <span className="inline-flex items-center gap-2 min-w-0" style={{ color: l.color }}>
      <span aria-hidden="true" className="w-2 h-2 flex-none" style={l.mark} />
      <span className="whitespace-nowrap">{l.label}</span>
      {detail && <span className="mono text-xs text-dim truncate">{detail}</span>}
    </span>
  );
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

export const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;
