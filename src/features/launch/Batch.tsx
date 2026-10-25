import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { api, type AccountSummary, type BulkStage, type Instance } from "../../api";
import { dismissBulk, useBulk, useInstances } from "../../data";
import { Icon } from "../../ui/Icon";
import { toastError } from "../../ui/Toast";
import { accountName, Avatar } from "../accounts/shared";
import { describeTarget } from "./target";

const STEPS = ["TKT", "RUN", "JOIN", "IN"];
type Kind = Exclude<BulkStage["kind"], "failed">;
/** The step in progress; 4 is all done, -1 none. */
const AT: Record<Kind, number> = { queued: -1, ticket: 0, starting: 1, joining: 2, inGame: 4, cancelled: -1 };
const NOTE: Record<Kind, string> = {
  queued: "queued",
  ticket: "getting ticket",
  starting: "starting client",
  joining: "joining server",
  inGame: "in game",
  cancelled: "cancelled",
};

function steps(stage: BulkStage, started: boolean, done: boolean) {
  // A started client that failed got past the ticket.
  const at = stage.kind === "failed" ? (started ? 1 : 0) : AT[stage.kind];
  const live = stage.kind === "failed" ? "step step-fail" : done ? "step step-live" : "step step-live pulse";
  return STEPS.map((_, i) => (i < at ? "step step-done" : i === at ? live : "step"));
}

/** A started client's stage from its tracked instance (as the backend mirrors it), so rows keep
 * moving after the batch itself stopped. */
function stageOf(i: Instance): BulkStage {
  switch (i.state) {
    case "starting":
    case "joining":
    case "inGame":
      return { kind: i.state };
    case "disconnected":
      return { kind: "failed", message: i.note ?? "Disconnected" };
    case "exited":
      return { kind: "failed", message: i.note ?? "Closed" };
  }
}

/** The running or last bulk launch, above Running clients. `onPick` picks accounts in Launch. */
export function BatchSection({ accounts, onPick }: { accounts: AccountSummary[]; onPick: (ids: number[]) => void }) {
  const qc = useQueryClient();
  const p = useBulk().data;
  const instances = useInstances().data;
  const [cancelling, setCancelling] = useState<string | null>(null);
  if (!p) return null;

  const byId = new Map(accounts.map((a) => [a.id, a]));
  const live = new Map(instances?.map((i) => [i.id, i]));
  const items = p.items.map((item) => {
    const inst = item.instanceId ? live.get(item.instanceId) : undefined;
    const running = item.stage.kind === "starting" || item.stage.kind === "joining" || item.stage.kind === "inGame";
    return { ...item, stage: inst && running ? stageOf(inst) : item.stage, job: inst?.jobId ?? null };
  });
  const n = items.length;
  const inGame = items.filter((i) => i.stage.kind === "inGame").length;
  const failed = items.filter((i) => i.stage.kind === "failed").length;
  const rest = items.filter((i) => i.stage.kind === "cancelled" || (i.stage.kind === "failed" && !i.instanceId)).map((i) => i.accountId);
  const target = describeTarget(p.target);
  const summary = [target.kind, p.sameServer === "pickFirst" ? "same server" : p.sameServer === "followFirst" ? "following the first" : null, `${p.delayMs / 1000} s`]
    .filter(Boolean)
    .join(" · ");
  const cancel = () => {
    setCancelling(p.batchId);
    api.bulkCancel(p.batchId).catch((e) => {
      setCancelling(null);
      toastError(e);
    });
  };

  return (
    <section aria-labelledby="batch-label" className="enter flex-none flex flex-col border-b border-line">
      <div className="flex flex-col gap-2.5 px-5 pt-[18px] pb-3.5 border-b border-line">
        <div className="flex items-center gap-2.5 min-w-0">
          <span id="batch-label" className="label">Batch</span>
          <span className="mono text-xs text-dim truncate" title={target.detail}>{summary}</span>
          {p.done ? (
            <button className="wc w-7 h-7 ml-auto" aria-label="Dismiss batch" title="Dismiss" onClick={() => dismissBulk(qc, p.batchId)}>
              <Icon name="x" size={10} />
            </button>
          ) : (
            <button className="btn btn-sm ml-auto" disabled={cancelling === p.batchId} onClick={cancel}>
              {cancelling === p.batchId ? "Cancelling…" : "Cancel"}
            </button>
          )}
        </div>
        <div className="flex items-baseline gap-2" role="status">
          <span className="mono text-[26px] font-medium">{inGame}/{n}</span>
          <span className="text-dim">
            in game{failed ? ` · ${failed} failed` : ""}
            {p.done ? " · finished" : ""}
          </span>
        </div>
        <div className="h-0.5 bg-line">
          <div className="h-0.5 bg-accent-text transition-[width] duration-500" style={{ width: `${(inGame / (n || 1)) * 100}%` }} />
        </div>
        {p.stopped && (
          <div className="flex flex-col items-start gap-2">
            <p role="alert" className="alert">{p.stopped}</p>
            {rest.length > 0 && (
              <button className="btn btn-sm" onClick={() => onPick(rest)}>
                Pick the {rest.length === 1 ? "account" : `${rest.length} accounts`} that didn't launch
              </button>
            )}
          </div>
        )}
        {p.note && <p className="text-xs text-dim">{p.note}</p>}
      </div>
      <div aria-hidden="true" className="mono grid grid-cols-[minmax(0,1fr)_132px] gap-x-4 px-5 pt-2 text-[9px] text-dim">
        <span />
        <span className="grid grid-cols-4 gap-[3px]">
          {STEPS.map((s) => <span key={s}>{s}</span>)}
        </span>
      </div>
      <ol className="scroll max-h-[40vh]">
        {items.map((item) => {
          const a = byId.get(item.accountId);
          const st = item.stage;
          const job = item.job;
          const note = st.kind === "failed" ? `failed · ${st.message}` : `${NOTE[st.kind]}${st.kind === "inGame" && job ? ` · job ${job.slice(0, 8)}` : ""}`;
          return (
            <li
              key={item.accountId}
              className={`grid grid-cols-[minmax(0,1fr)_132px] items-center gap-x-4 gap-y-1 px-5 py-3 border-b border-line last:border-b-0 ${st.kind === "cancelled" ? "opacity-55" : ""}`}
            >
              <span className="flex items-center gap-2.5 min-w-0">
                {a && <Avatar account={a} size={20} />}
                <span className="truncate font-medium">{a ? accountName(a) : `Account ${item.accountId}`}</span>
              </span>
              <span aria-hidden="true" className="grid grid-cols-4 gap-[3px]">
                {steps(st, !!item.instanceId, p.done).map((c, i) => <span key={i} className={c} />)}
              </span>
              <span className={`mono col-span-2 pl-[30px] text-xs ${st.kind === "failed" ? "text-orange" : st.kind === "inGame" ? "text-text" : "text-dim"}`}>
                {note}
              </span>
            </li>
          );
        })}
      </ol>
    </section>
  );
}
