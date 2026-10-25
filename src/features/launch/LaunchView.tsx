import { useEffect, useMemo, useRef, useState, type CSSProperties, type Dispatch, type SetStateAction } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api, asError, type AccountSummary, type ErrorPayload, type JoinCandidate, type LaunchTarget } from "../../api";
import { qk, useFavourites, usePresence } from "../../data";
import { Icon } from "../../ui/Icon";
import { friendly, toast } from "../../ui/Toast";
import { accountName, Avatar, Status } from "../accounts/shared";
import { FavouriteDialog } from "./Favourites";
import { JoinDialog } from "./JoinDialog";
import { describe, draftFor, draftForUser, placeOf, userFor, type Draft, type Tone } from "./target";

const LAST_KEY = "roalt.lastLaunchAccount";
const MARK: Record<Tone, CSSProperties> = {
  ok: { background: "var(--accent-text)" },
  info: { background: "var(--blue)" },
  bad: { background: "var(--orange)", transform: "rotate(45deg) scale(.85)" },
};
const LAUNCH_ERRORS: Record<string, string> = {
  cookieInvalid: "Sign in to this account again.",
  challenge: "Roblox wants a human check — open the account on roblox.com.",
  notFound: "That account or game no longer exists.",
};
const invalid = (message: string): ErrorPayload => ({ kind: "invalid", message });
/** One launch at a time, also across remounts of the view. */
let busy = false;

function lastUsed() {
  try {
    return Number(localStorage.getItem(LAST_KEY)) || null;
  } catch {
    return null;
  }
}
function remember(id: number) {
  try {
    localStorage.setItem(LAST_KEY, String(id));
  } catch {
    // Only a convenience.
  }
}

export function LaunchView({ accounts, selected, setSelected, draft, setDraft, step, setStep }: {
  accounts: AccountSummary[];
  selected: number | null;
  setSelected: (id: number) => void;
  draft: Draft;
  setDraft: Dispatch<SetStateAction<Draft>>;
  step: string | null;
  setStep: (step: string | null) => void;
}) {
  const qc = useQueryClient();
  const favourites = useFavourites();
  const [last] = useState(lastUsed);
  const pasted = useRef(false);
  const [joinOpen, setJoinOpen] = useState(false);
  const [saveOpen, setSaveOpen] = useState(false);

  const usable = (id: number | null) => accounts.find((a) => a.id === id && a.status === "valid") ?? null;
  const account = usable(selected) ?? usable(last);
  const validIds = useMemo(
    () => accounts.filter((a) => a.status === "valid").map((a) => a.id).sort((x, y) => x - y),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [accounts.map((a) => `${a.id}:${a.status}`).join()],
  );
  const presence = usePresence(validIds).data;

  // Debounced local parse; the result is kept with the text it belongs to.
  const { text } = draft;
  useEffect(() => {
    if (!text.trim() || draft.parsed?.text === text) return;
    const t = setTimeout(() => {
      api.targetParse(text).then(
        (value) => setDraft((d) => (d.text === text ? { ...d, parsed: { text, value } } : d)),
        () => {}, // launching parses again and shows the error
      );
    }, 150);
    return () => clearTimeout(t);
  }, [text]); // eslint-disable-line react-hooks/exhaustive-deps

  const parsed = text.trim() ? draft.parsed?.value ?? null : null;
  const place = placeOf(draft.place);
  const user = parsed?.kind === "username" ? userFor(draft, parsed.name) : null;
  const look = parsed && describe(parsed, place, user);
  const usableTarget = !!look && look.tone !== "bad" && !(parsed?.kind === "jobIdOnly" && place === null);
  const fav = draft.favId === null ? undefined : favourites.data?.find((f) => f.id === draft.favId);
  // A parse that hasn't caught up yet is launchable: resolveTarget parses again.
  const pending = !!text.trim() && draft.parsed?.text !== text;
  const canLaunch = !!account && (usableTarget || pending) && !step;

  /** What the draft launches; parses again if the debounce hasn't caught up, looks a username up. */
  async function resolveTarget(onLookup?: (name: string) => void): Promise<LaunchTarget> {
    const d = draft;
    const p = d.parsed?.text === d.text ? d.parsed.value : await api.targetParse(d.text);
    switch (p.kind) {
      case "target":
        return p.target;
      case "jobIdOnly": {
        const placeId = placeOf(d.place);
        if (placeId === null) throw invalid("Enter the place ID for this job ID.");
        return { kind: "server", placeId, jobId: p.jobId };
      }
      case "username": {
        let ref = userFor(d, p.name);
        if (!ref) {
          onLookup?.(p.name);
          ref = await api.userLookup(p.name);
          const found = { name: p.name, ref };
          setDraft((x) => (x.text === d.text ? { ...x, user: found } : x));
        }
        return { kind: "followUser", userId: ref.userId };
      }
      default:
        throw invalid(p.reason);
    }
  }

  async function launch() {
    if (!canLaunch || !account || busy) return;
    const a = account;
    busy = true;
    setStep("Getting a ticket…");
    let timer = 0;
    try {
      const target = await resolveTarget((name) => setStep(`Looking up @${name}…`));
      setStep("Getting a ticket…");
      timer = window.setTimeout(() => setStep("Starting Roblox…"), 800);
      const info = await api.launch(a.id, target, draft.favId ?? undefined);
      remember(a.id);
      if (draft.favId !== null) qc.invalidateQueries({ queryKey: qk.favourites });
      toast(`Launched ${accountName(a)}${info.viaBootstrapper ? " via a bootstrapper" : ""}`);
      if (info.warning) toast(info.warning, "error");
    } catch (e) {
      const err = asError(e);
      if (err.kind !== "cancelled") toast(LAUNCH_ERRORS[err.kind] ?? friendly(err), "error");
    } finally {
      clearTimeout(timer);
      busy = false;
      setStep(null);
    }
  }

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key === "Enter" && !document.querySelector("dialog[open]")) {
        e.preventDefault();
        void launch();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const join = (c: JoinCandidate) => {
    setJoinOpen(false);
    const p = c.presence;
    setDraft(
      p.kind === "inGame" && p.gameId && p.placeId !== null
        ? draftFor({ kind: "server", placeId: p.placeId, jobId: p.gameId }, null, `with @${c.username}`)
        : draftForUser({ userId: c.userId, username: c.username, displayName: c.displayName }),
    );
  };

  return (
    <main className="@container flex-1 min-w-0 flex flex-col">
      <div className="flex flex-wrap items-baseline gap-3 px-6 pt-[18px] pb-3">
        <h1 className="text-xl font-semibold tracking-[-0.01em]">Launch</h1>
        <span className="text-dim">Paste anything — the target type is detected.</span>
      </div>

      <div className="scroll flex-1 min-h-0">
        <div className="flex flex-col gap-6 max-w-[880px] px-6 pt-2 pb-6">
          <div className="flex flex-col gap-2">
            <div className="flex items-end justify-between gap-2">
              <label className="label" htmlFor="launch-target">Target</label>
              <button
                className="btn btn-sm"
                disabled={!account}
                title={account ? "A friend online, or one of your accounts" : "Pick an account first"}
                onClick={() => setJoinOpen(true)}
              >
                <Icon name="accounts" size={12} />
                Join someone
              </button>
            </div>
            <input
              id="launch-target"
              autoFocus
              className="field mono h-[38px]"
              placeholder="Paste a game link, place ID, job ID, private server or share link, or @username"
              spellCheck={false}
              autoComplete="off"
              value={text}
              onPaste={() => {
                pasted.current = true;
              }}
              onKeyDown={() => {
                pasted.current = false;
              }}
              onChange={(e) => {
                const v = e.target.value;
                setDraft((d) => ({ ...d, text: v, favId: null, note: null }));
              }}
            />
            {text.trim() && (
              <div className="enter flex items-center gap-3 min-h-[46px] px-3 py-1.5 border border-line bg-raised">
                <span
                  aria-hidden="true"
                  className={`w-2 h-2 flex-none ${look ? "" : "bg-dim pulse"}`}
                  style={look ? MARK[look.tone] : undefined}
                />
                <span className="flex items-baseline gap-3 min-w-0" aria-live="polite">
                  {look ? (
                    <>
                      <span className="font-semibold flex-none">{look.kind}</span>
                      <span className={`mono truncate ${look.tone === "bad" ? "text-orange" : "text-dim"}`} title={look.detail}>
                        {look.detail}
                      </span>
                    </>
                  ) : (
                    <span className="text-dim">Reading…</span>
                  )}
                </span>
                {draft.note && <span className="tag flex-none max-w-[30%] truncate">{draft.note}</span>}
                <button
                  className="btn btn-sm ml-auto flex-none"
                  disabled={!usableTarget}
                  style={fav ? { color: "var(--accent-text)" } : undefined}
                  onClick={() => setSaveOpen(true)}
                >
                  <Icon name="star" size={12} fill={!!fav} />
                  {fav ? "Edit favourite" : "Save as favourite"}
                </button>
              </div>
            )}
            {parsed?.kind === "jobIdOnly" && (
              <label className="enter flex flex-col gap-1.5 mt-2 max-w-[320px]">
                <span className="label">Place ID</span>
                <input
                  className="field mono"
                  ref={(el) => {
                    // Jump here after a pasted job ID, not while it is being typed.
                    if (el && pasted.current) {
                      pasted.current = false;
                      el.focus();
                    }
                  }}
                  inputMode="numeric"
                  spellCheck={false}
                  placeholder="The game's place ID or link"
                  value={draft.place}
                  aria-invalid={draft.place.trim() !== "" && place === null}
                  onChange={(e) => {
                    const v = e.target.value;
                    setDraft((d) => ({ ...d, place: v, favId: null, note: null }));
                  }}
                />
              </label>
            )}
          </div>

          <div className="flex flex-col gap-2">
            <div className="flex items-center justify-between">
              <span className="label" id="launch-account-label">Account</span>
              <span className="mono text-xs text-dim">{validIds.length} of {accounts.length} valid</span>
            </div>
            {accounts.length === 0 ? (
              <p className="text-dim">No accounts yet. Add one in Accounts.</p>
            ) : (
              <div role="radiogroup" aria-labelledby="launch-account-label" className="grid grid-cols-1 @xl:grid-cols-2 border-t border-l border-line">
                {accounts.map((a) => {
                  const valid = a.status === "valid";
                  return (
                    <label
                      key={a.id}
                      title={valid ? undefined : "Only an account with a valid cookie can launch"}
                      className={`flex items-center gap-2.5 h-[42px] px-3 border-r border-b border-line has-[:checked]:bg-sel ${valid ? "cursor-pointer hover:bg-hover" : "opacity-55"}`}
                    >
                      <input
                        type="radio"
                        name="launch-account"
                        className="cb"
                        checked={account?.id === a.id}
                        disabled={!valid}
                        onChange={() => setSelected(a.id)}
                      />
                      <Avatar account={a} size={22} />
                      <span className="flex-1 min-w-0 truncate">{accountName(a)}</span>
                      <span className="flex justify-end min-w-0 max-w-[50%] text-xs">
                        <Status status={a.status} presence={presence?.get(a.id)} />
                      </span>
                    </label>
                  );
                })}
              </div>
            )}
          </div>
        </div>
      </div>

      <div className="flex-none border-t border-line">
        <div className={step ? "live-bar" : "h-0.5"} />
        <div className="flex flex-wrap items-center gap-3 px-6 py-3.5">
          <span className="flex items-center gap-2 min-w-0 text-dim" aria-live="polite">
            {step ? (
              <>
                <span aria-hidden="true" className="w-2 h-2 flex-none bg-accent-text pulse" />
                <span className="text-text">{step}</span>
              </>
            ) : account ? (
              <span className="truncate">
                As <span className="text-text font-medium">{accountName(account)}</span> · <span className="kbd">Ctrl Enter</span>
              </span>
            ) : (
              "Pick an account with a valid cookie."
            )}
          </span>
          <button className="btn btn-primary h-10 px-5 ml-auto max-w-full" disabled={!canLaunch} title="Launch (Ctrl+Enter)" onClick={() => void launch()}>
            <Icon name="play" size={12} fill />
            <span className="truncate">{step ? "Launching…" : account ? `Launch ${accountName(account)}` : "Launch"}</span>
          </button>
        </div>
      </div>

      <JoinDialog open={joinOpen} onClose={() => setJoinOpen(false)} account={account} accounts={accounts} onPick={join} />
      <FavouriteDialog
        open={saveOpen}
        onClose={() => setSaveOpen(false)}
        accounts={accounts}
        initial={
          fav ?? {
            name: look ? `${look.kind} · ${look.detail}`.replace(/ · \w+ code ••••/, "").slice(0, 60) : "",
            accountIds: account ? [account.id] : [],
            inTray: false,
          }
        }
        resolve={() => (fav ? Promise.resolve(fav.target) : resolveTarget())}
        onSaved={(f) => setDraft((d) => (d.text === text ? { ...d, favId: f.id, note: f.name } : d))}
      />
    </main>
  );
}
