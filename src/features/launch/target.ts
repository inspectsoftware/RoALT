// The Launch composer's state and how a target reads in the UI. Shell owns the draft, so it
// survives switching views and is dropped with everything else when the vault locks.
import type { LaunchTarget, ParsedInput, UserRef } from "../../api";

export type Draft = {
  text: string;
  /** Place ID typed for a bare job ID. */
  place: string;
  /** The parse of `parsed.text`; lags `text` by the debounce. */
  parsed: { text: string; value: ParsedInput } | null;
  /** A username already looked up (`name` as typed), so launching need not ask again. */
  user: { name: string; ref: UserRef } | null;
  /** Filled from this favourite; typing clears it. */
  favId: number | null;
  /** Where the target came from (a favourite's name, "with @friend"); typing clears it. */
  note: string | null;
};

export const EMPTY_DRAFT: Draft = { text: "", place: "", parsed: null, user: null, favId: null, note: null };

const WEB = "https://www.roblox.com";

/** Text the parser reads back as the same target. */
export function targetText(t: LaunchTarget): string {
  switch (t.kind) {
    case "place":
      return t.launchData
        ? `${WEB}/games/start?placeId=${t.placeId}&launchData=${encodeURIComponent(t.launchData)}`
        : `${WEB}/games/${t.placeId}`;
    case "server":
      return `${WEB}/games/start?placeId=${t.placeId}&gameInstanceId=${t.jobId}`;
    case "privateLink":
      return `${WEB}/games/${t.placeId}?privateServerLinkCode=${t.linkCode}`;
    case "privateAccess":
      return `roblox://experiences/start?placeId=${t.placeId}&accessCode=${t.accessCode}`;
    case "shareLink":
      return `${WEB}/share?code=${t.code}&type=${t.linkType}`;
    case "shareLinkV2":
      return `${WEB}/share?code=${t.code}&v=v2`;
    case "universe":
      return `roblox://navigation/game_details?gameId=${t.universeId}`;
    case "followUser":
      return `roblox://experiences/start?userId=${t.userId}`;
  }
}

/** A draft holding a known target (no parse round trip needed). */
export function draftFor(target: LaunchTarget, favId: number | null = null, note: string | null = null): Draft {
  const text = targetText(target);
  return { ...EMPTY_DRAFT, text, parsed: { text, value: { kind: "target", target } }, favId, note };
}

/** A draft that joins this user (already known, so no lookup). */
export function draftForUser(ref: UserRef): Draft {
  const text = `@${ref.username}`;
  return { ...EMPTY_DRAFT, text, parsed: { text, value: { kind: "username", name: ref.username } }, user: { name: ref.username, ref } };
}

/** Digits, or a roblox.com game link. */
export function placeOf(s: string): number | null {
  const m = /^\s*(\d{1,19})\s*$/.exec(s) ?? /roblox\.com\/(?:[a-z]{2}(?:-[a-z0-9]{2,4})?\/)?games\/(\d{1,19})/i.exec(s);
  const n = m ? Number(m[1]) : 0;
  return n > 0 && Number.isSafeInteger(n) ? n : null;
}

export type Tone = "ok" | "info" | "bad";
export type Look = { kind: string; detail: string; tone: Tone };

const job = (j: string) => `job ${j.slice(0, 8)}…${j.slice(-4)}`;
const ok = (kind: string, detail: string): Look => ({ kind, detail, tone: "ok" });

/** "Game · place 1818"; link and access codes never show. */
export function describeTarget(t: LaunchTarget): Look {
  switch (t.kind) {
    case "place":
      return ok("Game", `place ${t.placeId}${t.launchData ? " · with launch data" : ""}`);
    case "server":
      return ok("Server", `place ${t.placeId} · ${job(t.jobId)}`);
    case "privateLink":
      return ok("Private server", `place ${t.placeId} · link code ••••`);
    case "privateAccess":
      return ok("Private server", `place ${t.placeId} · access code ••••`);
    case "shareLink":
      return ok("Share link", t.linkType);
    case "shareLinkV2":
      return ok("Share link", "opened when you launch");
    case "universe":
      return ok("Game", `universe ${t.universeId} · start place`);
    case "followUser":
      return ok("Join user", `user ${t.userId}`);
  }
}

export function describe(p: ParsedInput, place: number | null, user: UserRef | null): Look {
  switch (p.kind) {
    case "target":
      return describeTarget(p.target);
    case "jobIdOnly":
      return place === null
        ? { kind: "Server", detail: `${job(p.jobId)} · needs a place ID`, tone: "info" }
        : ok("Server", `place ${place} · ${job(p.jobId)}`);
    case "username":
      return { kind: "Join user", detail: `@${p.name}${user ? ` (${user.displayName})` : ""}`, tone: user ? "ok" : "info" };
    case "invalid":
      return { kind: "Not recognised", detail: p.reason, tone: "bad" };
    case "unsupported":
      return { kind: "Not supported", detail: p.reason, tone: "bad" };
  }
}

/** The looked-up user for this name, if any. */
export const userFor = (d: Draft, name: string) =>
  d.user && d.user.name.toLowerCase() === name.toLowerCase() ? d.user.ref : null;
