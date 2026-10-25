import { useEffect, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { api, asError, type AccountSummary, type JoinCandidate, type PresenceKind } from "../../api";
import { qk } from "../../data";
import { Dialog } from "../../ui/Dialog";
import { Icon } from "../../ui/Icon";
import { friendly } from "../../ui/Toast";
import { accountName, Avatar, PresenceStatus } from "../accounts/shared";

const RANK: Record<PresenceKind, number> = { inGame: 0, studio: 1, online: 2, unknown: 3, offline: 4 };

/** Friends online plus the other stored accounts; asked each time it opens, never polled. */
export function JoinDialog({ open, onClose, account, accounts, onPick }: {
  open: boolean;
  onClose: () => void;
  account: AccountSummary | null;
  accounts: AccountSummary[];
  onPick: (c: JoinCandidate) => void;
}) {
  const id = account?.id ?? 0;
  const list = useQuery({
    queryKey: [...qk.join, id],
    queryFn: () => api.joinCandidates(id),
    enabled: open && account !== null,
    staleTime: 0,
  });
  const [q, setQ] = useState("");
  useEffect(() => {
    if (open) setQ("");
  }, [open]);

  const needle = q.trim().toLowerCase();
  const shown = (list.data ?? [])
    .filter((c) => !needle || `${c.displayName}\n${c.username}`.toLowerCase().includes(needle))
    .sort((a, b) => RANK[a.presence.kind] - RANK[b.presence.kind]);

  return (
    <Dialog open={open} onClose={onClose} title={account ? `Join someone as ${accountName(account)}` : "Join someone"} width={480}>
      <div className="p-4 border-b border-line">
        <label className="flex items-center gap-2 h-[34px] px-2.5 border border-line-strong bg-bg text-dim focus-within:border-[var(--focus)]">
          <Icon name="search" size={14} />
          <span className="sr-only">Filter people</span>
          <input
            className="flex-1 min-w-0 bg-transparent outline-none text-text"
            placeholder="Filter by name"
            spellCheck={false}
            value={q}
            onChange={(e) => setQ(e.target.value)}
          />
        </label>
      </div>
      <div className="scroll max-h-[min(420px,60vh)]">
        {list.isError ? (
          <div className="flex flex-col items-start gap-3 p-5">
            <div role="alert" className="alert">{friendly(asError(list.error))}</div>
            <button className="btn btn-sm" onClick={() => list.refetch()} disabled={list.isFetching}>
              <Icon name="refresh" size={12} className={list.isFetching ? "spin" : ""} />
              Try again
            </button>
          </div>
        ) : !list.data ? (
          <div aria-busy="true" aria-label="Loading">
            {[0, 1, 2].map((i) => (
              <div key={i} className="flex items-center gap-3 h-[52px] px-4 border-b border-line">
                <span className="skel w-7 h-7" />
                <span className="skel h-4 w-40" />
              </div>
            ))}
          </div>
        ) : shown.length === 0 ? (
          <p className="p-5 text-dim">
            {needle ? "Nobody matches." : "No friends online, and no other account with a valid cookie."}
          </p>
        ) : (
          shown.map((c) => <Row key={c.userId} c={c} account={accounts.find((a) => a.id === c.accountId)} onPick={() => onPick(c)} />)
        )}
      </div>
      <p className="px-5 py-3 border-t border-line text-xs text-dim">
        Someone in a game whose server Roblox shows joins that server; anyone else is followed when you launch.
      </p>
    </Dialog>
  );
}

function Row({ c, account, onPick }: { c: JoinCandidate; account?: AccountSummary; onPick: () => void }) {
  return (
    <button className="flex items-center gap-3 w-full h-[52px] px-4 border-b border-line text-left hover:bg-hover" onClick={onPick}>
      {account ? (
        <Avatar account={account} size={28} />
      ) : (
        <span aria-hidden="true" className="w-7 h-7 flex-none grid place-content-center bg-line text-[10px] font-bold text-dim">
          {c.displayName.replace(/[^\p{L}\p{N}]/gu, "").slice(0, 2).toUpperCase() || "?"}
        </span>
      )}
      <span className="flex flex-col min-w-0 flex-1 leading-tight">
        <span className="font-semibold truncate">{c.displayName}</span>
        <span className="mono text-xs text-dim truncate">@{c.username}</span>
      </span>
      {account && <span className="tag flex-none">Your account</span>}
      <span className="flex justify-end min-w-0 max-w-[40%] text-xs">
        <PresenceStatus presence={c.presence} />
      </span>
    </button>
  );
}
