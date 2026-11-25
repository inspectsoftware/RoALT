import { useEffect, useRef, useState } from "react";
import { defaultRelayUrl, detectDeviceLabel, isLocalDashboard, type PairedPc } from "../storage";
import { pairWithManualCode, pairWithQr, type PairOffer } from "../transport";
import { nameKey } from "../validate";

export function PairView({
  qrHash,
  pairedPcs,
  onPaired,
  onCancel,
  canCancel,
}: {
  qrHash: { pairId: string; secretB64: string } | null;
  pairedPcs: PairedPc[];
  onPaired: (pc: PairedPc) => Promise<void>;
  onCancel?: () => void;
  canCancel: boolean;
}) {
  const [code, setCode] = useState("");
  const [label, setLabel] = useState(() => detectDeviceLabel());
  const [relayUrl, setRelayUrl] = useState(() => defaultRelayUrl());
  const [showRelay, setShowRelay] = useState(false);
  const [sas, setSas] = useState<string | null>(null);
  const [offer, setOffer] = useState<PairOffer | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const cancelRef = useRef<(() => void) | null>(null);
  const lookalike = offer && pairedPcs.find((p) => p.pcId !== offer.pc.pcId && nameKey(p.pcName) === nameKey(offer.pc.pcName));

  /** The user said yes: the PC enrolls this browser, then it is stored here. */
  const accept = async (o: PairOffer) => {
    setConfirming(true);
    setError(null);
    try {
      await onPaired(await o.confirm());
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
      setOffer(null);
      setSas(null);
    } finally {
      setConfirming(false);
    }
  };

  useEffect(() => {
    if (!qrHash) return;
    let active = true;
    setBusy(true);
    setError(null);
    setOffer(null);
    const task = pairWithQr(relayUrl, qrHash.pairId, qrHash.secretB64, label);
    cancelRef.current = task.cancel;
    task.promise
      .then((o) => {
        if (active) setOffer(o);
      })
      .catch((e: unknown) => {
        const msg = e instanceof Error ? e.message : String(e);
        if (active && msg !== "Cancelled") setError(msg);
      })
      .finally(() => {
        if (active) setBusy(false);
      });
    return () => {
      active = false;
      task.cancel();
    };
  }, [qrHash]); // eslint-disable-line react-hooks/exhaustive-deps

  const submitManual = async (e: React.FormEvent) => {
    e.preventDefault();
    const cleaned = code.trim().toUpperCase();
    if (cleaned.length < 6) return;
    setBusy(true);
    setError(null);
    setSas(null);
    const task = pairWithManualCode(relayUrl, cleaned, label.trim() || detectDeviceLabel(), (digits) => {
      setSas(digits);
    });
    cancelRef.current = task.cancel;
    try {
      setOffer(await task.promise);
    } catch (err: unknown) {
      const msg = err instanceof Error ? err.message : String(err);
      if (msg !== "Cancelled") setError(msg);
      setSas(null);
    } finally {
      setBusy(false);
    }
  };

  const cancelOffer = () => {
    cancelRef.current?.();
    cancelRef.current = null;
    setOffer(null);
    setSas(null);
    if (qrHash) onCancel?.();
  };

  return (
    <div className="mx-auto max-w-md px-4 py-10">
      <div className="glass-card rounded-2xl p-6 shadow-2xl">
        <div className="mb-5 flex items-center gap-3">
          <img src="/icon.svg" alt="RoALT" className="h-10 w-10 rounded-xl" />
          <div>
            <h1 className="text-lg font-semibold text-white">Pair a RoALT PC</h1>
            <p className="text-xs text-zinc-400">End-to-end encrypted · No account required</p>
          </div>
        </div>

        {offer ? (
          <div className="rounded-xl border border-lime-400/40 bg-lime-400/5 p-5 text-center">
            <div className="text-xs uppercase tracking-wider text-lime-300">{offer.sas ? "Compare with your PC" : "Approved on the PC"}</div>
            <div className="my-3 break-words text-xl font-semibold text-white">
              Pair with <bdi>{offer.pc.pcName}</bdi>?
            </div>
            {lookalike && (
              <div role="alert" className="mb-3 rounded-xl border border-amber-400/40 bg-amber-400/10 px-3.5 py-2.5 text-xs text-amber-200">
                This is not your paired PC <bdi>{lookalike.pcName}</bdi>, only a PC with a name that looks the same. Pair only if you
                expected a second PC with this name.
              </div>
            )}
            {offer.sas ? (
              <>
                <div className="my-2 font-mono text-3xl font-bold tracking-[0.2em] text-white">{offer.sas}</div>
                <p className="text-xs text-zinc-300">
                  Pair only if RoALT on your PC shows exactly this code. If the codes differ, or RoALT shows no code, cancel: someone
                  else answered.
                </p>
              </>
            ) : (
              <p className="text-xs text-zinc-300">Continue only if you opened this QR code on your own PC.</p>
            )}
            {error && <div className="mt-3 rounded-xl border border-red-500/30 bg-red-500/10 px-3.5 py-2.5 text-xs text-red-300">{error}</div>}
            <div className="mt-4 flex justify-center gap-2">
              <button type="button" onClick={cancelOffer} className="rounded-lg border border-white/15 px-4 py-2 text-xs text-zinc-300 hover:bg-white/5">
                Cancel
              </button>
              <button
                type="button"
                disabled={confirming}
                onClick={() => void accept(offer)}
                className="rounded-lg bg-lime-400 px-4 py-2 text-xs font-semibold text-zinc-950 hover:bg-lime-300 disabled:opacity-40"
              >
                {confirming ? "Pairing…" : offer.sas ? "Codes match, pair" : "Pair"}
              </button>
            </div>
          </div>
        ) : sas ? (
          <div className="rounded-xl border border-lime-400/40 bg-lime-400/5 p-5 text-center">
            <div className="text-xs uppercase tracking-wider text-lime-300">Verify on your PC</div>
            <div className="my-3 font-mono text-4xl font-bold tracking-[0.2em] text-white">{sas}</div>
            <p className="text-xs text-zinc-300">
              Check that this 6-digit code matches the dialog in RoALT on your PC, then click <strong>Approve</strong> there. You confirm here
              next.
            </p>
            <button type="button" onClick={cancelOffer} className="mt-4 rounded-lg border border-white/15 px-4 py-2 text-xs text-zinc-300 hover:bg-white/5">
              Cancel
            </button>
          </div>
        ) : qrHash && busy ? (
          <div className="py-8 text-center">
            <div className="mx-auto mb-3 h-7 w-7 animate-spin rounded-full border-2 border-lime-400 border-t-transparent" />
            <div className="text-sm font-medium text-white">Waiting for approval on your PC…</div>
            <p className="mt-1 text-xs text-zinc-400">Click Approve in RoALT on the PC that shows the QR code.</p>
            <p className="mt-2 text-xs text-amber-300">
              RoALT should say this request <strong>scanned the QR code</strong>, with no 6-digit code. If it shows a 6-digit code or says
              the code was typed, click Deny there: someone else answered.
            </p>
            <button type="button" onClick={cancelOffer} className="mt-4 rounded-lg border border-white/15 px-4 py-2 text-xs text-zinc-300 hover:bg-white/5">
              Cancel
            </button>
          </div>
        ) : (
          <form onSubmit={(e) => void submitManual(e)} className="space-y-4">
            <p className="text-sm text-zinc-300">
              Open <strong>RoALT → Settings → Remote → Pair device</strong> on your Windows PC, then scan the QR code or enter the 8-character code below:
            </p>

            <div>
              <label className="mb-1 block text-xs font-medium text-zinc-400">Pairing code</label>
              <input
                type="text"
                value={code}
                onChange={(e) => setCode(e.target.value.toUpperCase())}
                placeholder="ABCD-1234"
                autoComplete="off"
                className="w-full rounded-xl border border-white/10 bg-black/40 px-4 py-3 text-center font-mono text-xl font-semibold tracking-[0.18em] text-white placeholder:text-zinc-600 focus:border-lime-400 focus:outline-none"
              />
            </div>

            <div>
              <label className="mb-1 block text-xs font-medium text-zinc-400">This device's name</label>
              <input
                type="text"
                value={label}
                onChange={(e) => setLabel(e.target.value)}
                maxLength={64}
                className="w-full rounded-xl border border-white/10 bg-black/40 px-3.5 py-2.5 text-sm text-white focus:border-lime-400 focus:outline-none"
              />
            </div>

            {isLocalDashboard() && (
              <div>
                <button type="button" onClick={() => setShowRelay((v) => !v)} className="text-xs text-zinc-500 hover:text-zinc-300">
                  {showRelay ? "Hide relay URL" : "Custom relay URL…"}
                </button>
                {showRelay && (
                  <input
                    type="text"
                    value={relayUrl}
                    onChange={(e) => setRelayUrl(e.target.value)}
                    className="mt-2 w-full rounded-lg border border-white/10 bg-black/40 px-3 py-2 font-mono text-xs text-zinc-200 focus:border-lime-400 focus:outline-none"
                  />
                )}
              </div>
            )}

            {error && (
              <div className="rounded-xl border border-red-500/30 bg-red-500/10 px-3.5 py-2.5 text-xs text-red-300">
                {error}
              </div>
            )}

            <div className="flex items-center gap-2 pt-1">
              {canCancel && onCancel && (
                <button
                  type="button"
                  onClick={onCancel}
                  className="rounded-xl border border-white/10 px-4 py-2.5 text-sm font-medium text-zinc-300 hover:bg-white/5"
                >
                  Back
                </button>
              )}
              <button
                type="submit"
                disabled={busy || code.trim().length < 6}
                className="flex-1 rounded-xl bg-lime-400 px-4 py-2.5 text-sm font-semibold text-zinc-950 transition hover:bg-lime-300 disabled:opacity-40"
              >
                {busy ? "Connecting…" : "Pair device"}
              </button>
            </div>
          </form>
        )}

        <div className="mt-5 border-t border-white/5 pt-4 text-[11px] leading-relaxed text-zinc-500">
          Pairing keys stay in this browser, and commands and previews are end-to-end encrypted. The relay still sees which PCs this browser
          connects to and when, and the server that hosts this page could change its code. Cookies and vault passwords never leave your PC.
        </div>
      </div>
    </div>
  );
}
