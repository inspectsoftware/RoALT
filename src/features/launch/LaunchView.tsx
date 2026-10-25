import { useEffect, useMemo, useRef, useState, type CSSProperties, type Dispatch, type SetStateAction } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api, asError, type AccountSummary, type ErrorPayload, type JoinCandidate, type LaunchTarget, type SameServer, type Settings } from "../../api";
import { qk, useBulk, useFavourites, useMultiStatus, usePresence, useSetSettings, useSettings } from "../../data";
import { NumberField, Segmented, Switch } from "../../ui/controls";
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

export function lastLaunchAccount() {
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

export function LaunchView({ accounts, picked, setPicked, sameServer, setSameServer, draft, setDraft, step, setStep }: {
  accounts: AccountSummary[];
  picked: number[];
  setPicked: Dispatch<SetStateAction<number[]>>;
  sameServer: SameServer;
  setSameServer: (s: SameServer) => void;
  draft: Draft;
  setDraft: Dispatch<SetStateAction<Draft>>;
  step: string | null;
  setStep: (step: string | null) => void;
}) {
  const qc = useQueryClient();
  const favourites = useFavourites();
  const multi = useMultiStatus().data;
  const batch = useBulk().data;
  const delayMs = useSettings().data?.launchDelayMs ?? 6000;
  const setSettings = useSetSettings();
  const pasted = useRef(false);
  const [joinOpen, setJoinOpen] = useState(false);
  const [saveOpen, setSaveOpen] = useState(false);

  // In list order, which is also the bulk launch order.
  const chosen = accounts.filter((a) => a.status === "valid" && picked.includes(a.id));
  const account = chosen[0] ?? null;
  const bulk = chosen.length > 1;
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
  const blocked = !bulk
    ? null
    : !multi?.active
      ? (multi?.enabled && multi.message) || "Turn on multi-instance to launch several accounts."
      : batch && !batch.done
        ? "A batch is still running."
        : null;
  const canLaunch = !!account && (usableTarget || pending) && !step && !blocked;

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
    const ids = chosen.map((x) => x.id);
    const first = ids.length > 1 ? "Starting the batch…" : "Getting a ticket…";
    busy = true;
    setStep(first);
    let timer = 0;
    try {
      const target = await resolveTarget((name) => setStep(`Looking up @${name}…`));
      setStep(first);
      if (ids.length > 1) {
        // From the cache: a stagger committed by this click's blur hasn't re-rendered yet.
        const delay = qc.getQueryData<Settings>(qk.settings)?.launchDelayMs ?? delayMs;
        await api.bulkLaunch({ accountIds: ids, target, delayMs: delay, sameServer, favouriteId: draft.favId });
      } else {
        timer = window.setTimeout(() => setStep("Starting Roblox…"), 800);
        const info = await api.launch(a.id, target, draft.favId ?? undefined);
        remember(a.id);
        toast(`Launched ${accountName(a)}${info.viaBootstrapper ? " via a bootstrapper" : ""}`);
        if (info.warning) toast(info.warning, "error");
      }
      if (draft.favId !== null) qc.invalidateQueries({ queryKey: qk.favourites });
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
    <main className="@container flex-[999_1_520px] min-w-0 flex flex-col">
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
              <span className="label" id="launch-account-label">Accounts</span>
              <span className="mono text-xs text-dim">{chosen.length} of {validIds.length} valid</span>
            </div>
            {accounts.length === 0 ? (
              <p className="text-dim">No accounts yet. Add one in Accounts.</p>
            ) : (
              <div role="group" aria-labelledby="launch-account-label" className="grid grid-cols-1 @xl:grid-cols-2 border-t border-l border-line">
                {accounts.map((a) => {
                  const valid = a.status === "valid";
                  return (
                    <label
                      key={a.id}
                      title={valid ? undefined : "Only an account with a valid cookie can launch"}
                      className={`flex items-center gap-2.5 h-[42px] px-3 border-r border-b border-line has-[:checked]:bg-sel ${valid ? "cursor-pointer hover:bg-hover" : "opacity-55"}`}
                    >
                      <input
                        type="checkbox"
                        className="cb"
                        checked={valid && picked.includes(a.id)}
                        disabled={!valid}
                        onChange={(e) => {
                          const on = e.target.checked;
                          setPicked((p) => (on ? [...p, a.id] : p.filter((id) => id !== a.id)));
                        }}
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
        {bulk && (
          <div className="enter flex flex-wrap items-center gap-x-6 gap-y-2 px-6 pt-3.5">
            <NumberField
              label="Stagger"
              unit="seconds"
              short="s"
              min={2}
              max={60}
              step={0.5}
              value={delayMs / 1000}
              onCommit={(s) => setSettings.mutate({ launchDelayMs: Math.round(s * 1000) })}
            />
            <span className="flex items-center gap-2.5">
              <Switch label="Same server" checked={sameServer !== "off"} onChange={(on) => setSameServer(on ? "pickFirst" : "off")} />
              <span aria-hidden="true">Same server</span>
            </span>
            {sameServer !== "off" && (
              <Segmented
                label="How to find the server"
                value={sameServer}
                options={[{ value: "pickFirst", label: "Pick a server" }, { value: "followFirst", label: "Follow the first" }]}
                onChange={setSameServer}
              />
            )}
          </div>
        )}
        <div className="flex flex-wrap items-center gap-3 px-6 py-3.5">
          <span className="flex items-center gap-2 min-w-0 text-dim" aria-live="polite">
            {step ? (
              <>
                <span aria-hidden="true" className="w-2 h-2 flex-none bg-accent-text pulse" />
                <span className="text-text">{step}</span>
              </>
            ) : blocked ? (
              <span className="alert">{blocked}</span>
            ) : account ? (
              <span className="truncate">
                As <span className="text-text font-medium">{chosen.map(accountName).join(", ")}</span> · <span className="kbd">Ctrl Enter</span>
              </span>
            ) : (
              "Pick an account with a valid cookie."
            )}
          </span>
          <button className="btn btn-primary h-10 px-5 ml-auto max-w-full" disabled={!canLaunch} title="Launch (Ctrl+Enter)" onClick={() => void launch()}>
            <Icon name="play" size={12} fill />
            <span className="truncate">
              {step ? "Launching…" : bulk ? `Launch ${chosen.length} accounts` : account ? `Launch ${accountName(account)}` : "Launch"}
            </span>
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
            accountIds: chosen.map((a) => a.id),
            inTray: false,
          }
        }
        resolve={() => (fav ? Promise.resolve(fav.target) : resolveTarget())}
        onSaved={(f) => setDraft((d) => (d.text === text ? { ...d, favId: f.id, note: f.name } : d))}
      />
    </main>
  );
}
