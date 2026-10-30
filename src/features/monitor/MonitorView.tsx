import { useEffect, useState, type ReactNode } from "react";
import type { AccountSummary, ClientMetrics } from "../../api";
import { useInstances, useMetrics } from "../../data";
import { accountName, Avatar } from "../accounts/shared";
import { isRunning, STATES, uptime, useNow } from "../launch/Running";
import { fmtNum, t } from "../../i18n";

/** Samples kept per line: two minutes at one every 2 s. */
const KEEP = 60;

type Series = { cpu: number[]; mem: number[] };

const one = (n: number) => fmtNum(n, { minimumFractionDigits: 1, maximumFractionDigits: 1 });
export const mb = (n: number) => (n >= 1024 ? t("monitor.gb", { n: one(n / 1024) }) : t("monitor.mb", { n: fmtNum(Math.round(n)) }));
export const pct = (n: number | null) => (n === null ? "—" : t("monitor.pct", { n: n < 10 ? one(n) : fmtNum(Math.round(n)) }));
function rate(bps: number | null) {
  if (bps === null) return "—";
  if (bps >= 1024 * 1024) return t("monitor.mbps", { n: one(bps / 1024 / 1024) });
  if (bps >= 1024) return t("monitor.kbps", { n: fmtNum(Math.round(bps / 1024)) });
  return t("monitor.bps", { n: fmtNum(Math.round(bps)) });
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
  const range = t("monitor.range", { label, min: format(Math.min(...values)), max: format(Math.max(...values)), m: Math.round((values.length * 2) / 60) || 1, now: format(last) });
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
  const gpuMemNow = list.some((m) => m.gpuMemMb !== null) ? list.reduce((n, m) => n + (m.gpuMemMb ?? 0), 0) : null;

  return (
    <main className="scroll flex-1 min-w-0 px-10 pt-6 pb-12">
      <div className="enter max-w-[1000px] flex flex-col">
        <h1 className="text-xl font-semibold tracking-[-0.01em]">{t("monitor.title")}</h1>
        <p className="text-dim mt-1">{t("monitor.intro")}</p>

        <div className="grid grid-cols-2 lg:grid-cols-5 border border-line mt-5">
          <Tile label={t("monitor.clients")} value={String(running.length)} />
          <Tile label={t("monitor.cpuAll")} value={pct(cpuNow)}>
            {list.length > 0 && <Sparkline values={hist.totals.cpu} floor={10} zero label={t("monitor.cpuAll")} format={pct} />}
          </Tile>
          <Tile label={t("monitor.privateMemory")} value={mb(memNow)}>
            {list.length > 0 && <Sparkline values={hist.totals.mem} floor={64} label={t("monitor.privateMemoryAll")} format={mb} />}
          </Tile>
          <Tile label={t("monitor.gpuMemory")} value={gpuMemNow === null ? "—" : mb(gpuMemNow)} />
          <Tile label={t("monitor.io")} value={rate(ioNow)} />
        </div>

        {running.length === 0 ? (
          <p className="text-dim mt-6">{t("monitor.empty")}</p>
        ) : (
          <table className="mono w-full border-collapse text-xs mt-6">
            <thead>
              <tr className="h-8 text-dim text-left">
                <th className="font-normal border-b border-line">{t("monitor.account")}</th>
                <th className="font-normal border-b border-line">{t("monitor.state")}</th>
                <th className="font-normal border-b border-line text-right pr-3">{t("monitor.cpu")}</th>
                <th className="font-normal border-b border-line hidden xl:table-cell"><span className="sr-only">{t("monitor.cpuTrend")}</span></th>
                <th className="font-normal border-b border-line text-right pr-3" title={t("monitor.memoryHint")}>{t("monitor.memory")}</th>
                <th className="font-normal border-b border-line hidden xl:table-cell"><span className="sr-only">{t("monitor.memoryTrend")}</span></th>
                <th className="font-normal border-b border-line text-right pr-3" title={t("monitor.gpuHint")}>{t("monitor.gpu")}</th>
                <th className="font-normal border-b border-line text-right pr-3" title={t("monitor.gpuMemoryHint")}>{t("monitor.gpuMem")}</th>
                <th className="font-normal border-b border-line text-right pr-3">{t("monitor.read")}</th>
                <th className="font-normal border-b border-line text-right pr-3">{t("monitor.write")}</th>
                <th className="font-normal border-b border-line text-right pr-3">{t("monitor.up")}</th>
                <th className="font-normal border-b border-line text-right" title={t("monitor.restartsHint")}>{t("monitor.restarts")}</th>
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
                        <span className="truncate">{a ? accountName(a) : t("monitor.accountId", { id: i.accountId })}</span>
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
                    <td className="pr-3 hidden xl:table-cell"><Sparkline values={h?.cpu ?? []} floor={10} zero label={t("monitor.cpu")} format={pct} /></td>
                    <td className="pr-3 text-right" title={m ? t("monitor.workingSet", { mb: mb(m.workingSetMb) }) : undefined}>{m ? mb(m.privateMb) : "—"}</td>
                    <td className="pr-3 hidden xl:table-cell"><Sparkline values={h?.mem ?? []} floor={64} label={t("monitor.privateMemory")} format={mb} /></td>
                    <td className="pr-3 text-right whitespace-nowrap">{m ? pct(m.gpuPct) : "—"}</td>
                    <td className="pr-3 text-right whitespace-nowrap">{m && m.gpuMemMb !== null ? mb(m.gpuMemMb) : "—"}</td>
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
