import { useEffect, useState, type ReactNode } from "react";
import type { AccountSummary, ClientMetrics } from "../../api";
import { useInstances, useMetrics } from "../../data";
import { accountName, Avatar } from "../accounts/shared";
import { isRunning, STATES, uptime, useNow } from "../launch/Running";

/** Samples kept per line: two minutes at one every 2 s. */
const KEEP = 60;

type Series = { cpu: number[]; mem: number[] };

export const mb = (n: number) => (n >= 1024 ? `${(n / 1024).toFixed(1)} GB` : `${Math.round(n)} MB`);
export const pct = (n: number | null) => (n === null ? "—" : `${n < 10 ? n.toFixed(1) : Math.round(n)}%`);
function rate(bps: number | null) {
  if (bps === null) return "—";
  if (bps >= 1024 * 1024) return `${(bps / 1024 / 1024).toFixed(1)} MB/s`;
  if (bps >= 1024) return `${Math.round(bps / 1024)} KB/s`;
  return `${Math.round(bps)} B/s`;
}
const push = (list: number[], v: number) => [...list, v].slice(-KEEP);

/** One series over the last two minutes: a 2px line with a faint wash under it, the latest
 * value marked. Its title reads out the range, the table cell beside it the current value. */
function Sparkline({ values, floor, zero, label, format }: {
  values: number[];
  /** The smallest span the height stands for, so noise stays flat. */
  floor: number;
  /** From 0 (a share); else from the lowest value (memory: its change is the story). */
  zero?: boolean;
  label: string;
  format: (n: number) => string;
}) {
  const w = 96;
  const h = 24;
  const pad = 5;
  if (values.length < 2) return <svg width={w} height={h} aria-hidden="true" />;
  const base = zero ? 0 : Math.min(...values);
  const max = Math.max(base + floor, ...values);
  const x = (i: number) => pad + (i / (KEEP - 1)) * (w - 2 * pad);
  const offset = KEEP - values.length;
  const y = (v: number) => h - pad - ((v - base) / (max - base)) * (h - 2 * pad);
  const pts = values.map((v, i) => `${x(i + offset).toFixed(1)},${y(v).toFixed(1)}`);
  const last = values[values.length - 1];
  const range = `${label}: ${format(Math.min(...values))} to ${format(Math.max(...values))} over the last ${Math.round((values.length * 2) / 60) || 1} min, now ${format(last)}`;
  return (
    <svg width={w} height={h} role="img" aria-label={range} className="flex-none">
      <title>{range}</title>
      <path d={`M${x(offset)},${h - pad} L${pts.join(" L")} L${x(KEEP - 1)},${h - pad} Z`} fill="var(--accent-text)" opacity={0.1} />
      <polyline points={pts.join(" ")} fill="none" stroke="var(--accent-text)" strokeWidth={2} strokeLinejoin="round" strokeLinecap="round" />
      <circle cx={x(KEEP - 1)} cy={y(last)} r={4} fill="var(--accent-text)" stroke="var(--bg)" strokeWidth={2} />
    </svg>
  );
}

function Tile({ label, value, children }: { label: string; value: string; children?: ReactNode }) {
  return (
    <div className="flex flex-col gap-1 px-5 py-4 border-r border-line last:border-r-0 min-w-0">
      <span className="label">{label}</span>
      <div className="flex items-end justify-between gap-3">
        <span className="text-[22px] font-semibold leading-none whitespace-nowrap">{value}</span>
        {children}
      </div>
    </div>
  );
}

/** Live CPU, memory and IO of every client RoALT runs, polled every 2 s. */
export function MonitorView({ accounts }: { accounts: AccountSummary[] }) {
  const running = (useInstances().data ?? []).filter(isRunning);
  const metrics = useMetrics();
  const now = useNow(30_000);
  const [hist, setHist] = useState<{ per: ReadonlyMap<string, Series>; totals: Series }>({ per: new Map(), totals: { cpu: [], mem: [] } });
  const byId = new Map(accounts.map((a) => [a.id, a]));
  const list = metrics.data ?? [];
  const byInst = new Map<string, ClientMetrics>(list.map((m) => [m.instanceId, m]));

  // One point per new sample; clients that ended drop out.
  const { data, dataUpdatedAt } = metrics;
  useEffect(() => {
    if (!data) return;
    setHist((old) => {
      const per = new Map<string, Series>();
      for (const m of data) {
        const s = old.per.get(m.instanceId) ?? { cpu: [], mem: [] };
        per.set(m.instanceId, { cpu: m.cpuPct === null ? s.cpu : push(s.cpu, m.cpuPct), mem: push(s.mem, m.privateMb) });
      }
      const cpu = data.reduce((n, m) => n + (m.cpuPct ?? 0), 0);
      const mem = data.reduce((n, m) => n + m.privateMb, 0);
      if (!data.length) return { per, totals: { cpu: [], mem: [] } };
      return { per, totals: { cpu: push(old.totals.cpu, cpu), mem: push(old.totals.mem, mem) } };
    });
  }, [data, dataUpdatedAt]);

  const cpuNow = list.some((m) => m.cpuPct !== null) ? list.reduce((n, m) => n + (m.cpuPct ?? 0), 0) : null;
  const memNow = list.reduce((n, m) => n + m.privateMb, 0);
  const ioNow = list.some((m) => m.readBps !== null) ? list.reduce((n, m) => n + (m.readBps ?? 0) + (m.writeBps ?? 0), 0) : null;

  return (
    <main className="scroll flex-1 min-w-0 px-10 pt-6 pb-12">
      <div className="enter max-w-[1000px] flex flex-col">
        <h1 className="text-xl font-semibold tracking-[-0.01em]">Monitor</h1>
        <p className="text-dim mt-1">What each client RoALT runs uses, every 2 seconds. Limits are under each client in Launch.</p>

        <div className="grid grid-cols-2 lg:grid-cols-4 border border-line mt-5">
          <Tile label="Clients" value={String(running.length)} />
          <Tile label="CPU, all clients" value={pct(cpuNow)}>
            {list.length > 0 && <Sparkline values={hist.totals.cpu} floor={10} zero label="CPU, all clients" format={pct} />}
          </Tile>
          <Tile label="Private memory" value={mb(memNow)}>
            {list.length > 0 && <Sparkline values={hist.totals.mem} floor={64} label="Private memory, all clients" format={mb} />}
          </Tile>
          <Tile label="IO" value={rate(ioNow)} />
        </div>

        {running.length === 0 ? (
          <p className="text-dim mt-6">Clients you launch show here while they run.</p>
        ) : (
          <table className="mono w-full border-collapse text-xs mt-6">
            <thead>
              <tr className="h-8 text-dim text-left">
                <th className="font-normal border-b border-line">Account</th>
                <th className="font-normal border-b border-line">State</th>
                <th className="font-normal border-b border-line text-right pr-3">CPU</th>
                <th className="font-normal border-b border-line"><span className="sr-only">CPU trend</span></th>
                <th className="font-normal border-b border-line text-right pr-3" title="Private memory (what a leak grows); working set in the tooltip">Memory</th>
                <th className="font-normal border-b border-line"><span className="sr-only">Memory trend</span></th>
                <th className="font-normal border-b border-line text-right pr-3">Read</th>
                <th className="font-normal border-b border-line text-right pr-3">Write</th>
                <th className="font-normal border-b border-line text-right pr-3">Up</th>
                <th className="font-normal border-b border-line text-right" title="Rejoins and restarts that led to this client">Restarts</th>
              </tr>
            </thead>
            <tbody>
              {running.map((i) => {
                const a = byId.get(i.accountId);
                const m = byInst.get(i.id);
                const h = hist.per.get(i.id);
                const [label, color] = STATES[i.state];
                return (
                  <tr key={i.id} className="h-11 border-b border-line" title={`PID ${i.pid}`} style={{ fontVariantNumeric: "tabular-nums" }}>
                    <td className="pr-3 max-w-0 w-[24%]">
                      <span className="flex items-center gap-2 min-w-0 font-sans">
                        {a && <Avatar account={a} size={20} />}
                        <span className="truncate">{a ? accountName(a) : `Account ${i.accountId}`}</span>
                      </span>
                    </td>
                    <td className="pr-3 whitespace-nowrap">
                      <span className="flex items-baseline gap-1.5">
                        <span aria-hidden="true" className="w-1.5 h-1.5 flex-none" style={{ background: color }} />
                        {label}
                        {i.note && <span className="text-orange truncate"> · {i.note}</span>}
                      </span>
                    </td>
                    <td className="pr-3 text-right">{m ? pct(m.cpuPct) : "—"}</td>
                    <td className="pr-3"><Sparkline values={h?.cpu ?? []} floor={10} zero label="CPU" format={pct} /></td>
                    <td className="pr-3 text-right" title={m ? `Working set ${mb(m.workingSetMb)}` : undefined}>{m ? mb(m.privateMb) : "—"}</td>
                    <td className="pr-3"><Sparkline values={h?.mem ?? []} floor={64} label="Private memory" format={mb} /></td>
                    <td className="pr-3 text-right whitespace-nowrap">{m ? rate(m.readBps) : "—"}</td>
                    <td className="pr-3 text-right whitespace-nowrap">{m ? rate(m.writeBps) : "—"}</td>
                    <td className="pr-3 text-right whitespace-nowrap">{uptime(i.startedAt, now)}</td>
                    <td className="text-right">{i.rejoins}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>
    </main>
  );
}
