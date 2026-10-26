import { useEffect, useLayoutEffect, useMemo, useReducer, useRef, useState, type PointerEvent, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api, type AccountStatus, type AccountSummary, type Density, type Group, type Presence, type VaultMode } from "../../api";
import { groupColor, qk, setAccounts, usePresence, useSetSettings, useUpdateAccount } from "../../data";
import { Segmented } from "../../ui/controls";
import { ConfirmDialog } from "../../ui/Dialog";
import { Icon } from "../../ui/Icon";
import { toast } from "../../ui/Toast";
import { DetailPanel } from "./DetailPanel";
import { accountName, ago, Avatar, plural, Status } from "./shared";
import { Divider, usePanelWidth } from "../../ui/Resizer";

type Filter = "all" | "inGame" | "online" | "offline" | Exclude<AccountStatus, "valid"> | "fav";
// The chips match what the Status column shows.
const FILTERS: { key: Filter; label: string }[] = [
  { key: "all", label: "All" },
  { key: "inGame", label: "In game" },
  { key: "online", label: "Online" },
  { key: "offline", label: "Offline" },
  { key: "invalid", label: "Cookie expired" },
  { key: "challenged", label: "Needs a check" },
  { key: "unknown", label: "Not checked" },
  { key: "fav", label: "Favourites" },
];

/** What the Status cell shows: presence for a valid cookie (Studio counts as online), else the cookie status. */
const shownKey = (a: AccountSummary, p?: Presence): Filter | "valid" => {
  if (a.status !== "valid") return a.status;
  if (!p || p.kind === "unknown") return "valid";
  return p.kind === "studio" ? "online" : p.kind;
};
const inFilter = (a: AccountSummary, f: Filter, p?: Presence) =>
  f === "all" || (f === "fav" ? a.favourite : shownKey(a, p) === f);
const isTyping = (t: EventTarget | null) =>
  t instanceof HTMLElement &&
  (t.matches("textarea, select, input:not([type=checkbox]):not([type=radio])") || t.isContentEditable);
// Cookies are checked once per app session, when the list first shows (after unlock in master mode).
let validatedThisSession = false;
const reducedMotion = () => matchMedia("(prefers-reduced-motion: reduce)").matches;

export function AccountsView({ accounts, groups, density, vaultMode, clipboardSeconds, group, setGroup, selected, setSelected, onAdd, onLaunch }: {
  accounts: AccountSummary[];
  groups: Group[];
  density: Density;
  vaultMode: VaultMode;
  clipboardSeconds: number;
  group: number | null;
  setGroup: (id: number | null) => void;
  selected: number | null;
  setSelected: (id: number | null) => void;
  onAdd: () => void;
  onLaunch: (id: number) => void;
}) {
  const detail = usePanelWidth("detail", 340, 280, 560);
  const qc = useQueryClient();
  const update = useUpdateAccount();
  const setSettings = useSetSettings();
  const [q, setQ] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [checked, setChecked] = useState<Set<number>>(new Set());
  const [removeIds, setRemoveIds] = useState<number[]>([]);
  const [removeOpen, setRemoveOpen] = useState(false);
  const [drag, setDrag] = useState<{ id: number; over: number } | null>(null);
  const [order, setOrder] = useState<number[] | null>(null);
  const [now, tick] = useReducer(() => Date.now(), Date.now());
  const tbody = useRef<HTMLTableSectionElement>(null);
  const dragging = useRef<{ id: number; startY: number; row: HTMLElement; over: number } | null>(null);
  const flip = useRef<Map<number, number> | null>(null);
  const seen = useRef(new Set<number>());
  const compact = density === "compact";

  useEffect(() => {
    const t = setInterval(tick, 60_000);
    return () => clearInterval(t);
  }, []);
  // A local order bridges the gap until the optimistic cache update arrives.
  useEffect(() => setOrder(null), [accounts]);

  const byId = useMemo(() => new Map(accounts.map((a) => [a.id, a])), [accounts]);
  const groupById = useMemo(() => new Map(groups.map((g) => [g.id, g])), [groups]);
  const ordered = order ? order.flatMap((id) => byId.get(id) ?? []) : accounts;
  // A group deleted while it was the filter means no filter.
  const inGroup = group === null || !groupById.has(group) ? ordered : ordered.filter((a) => a.groupIds.includes(group));
  const needle = q.trim().toLowerCase();
  const matching = inGroup.filter(
    (a) => !needle || `${a.alias}\n${a.displayName}\n@${a.username}\n${a.userId}`.toLowerCase().includes(needle),
  );
  // Presence only for rows that can be on screen, and only for valid cookies (DESIGN: visible rows only).
  const presenceIds = useMemo(
    () => matching.filter((a) => a.status === "valid").map((a) => a.id).sort((x, y) => x - y),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [matching.map((a) => `${a.id}:${a.status}`).join()],
  );
  const presence = usePresence(presenceIds).data;
  const shown = matching.filter((a) => inFilter(a, filter, presence?.get(a.id)));
  const checkedIds = accounts.filter((a) => checked.has(a.id)).map((a) => a.id);
  const current = (selected !== null && byId.get(selected)) || shown[0] || null;
  const needsSignIn = accounts.filter((a) => a.status === "invalid").length;
  const lastChecked = Math.max(0, ...accounts.map((a) => a.statusCheckedAt ?? 0)) || null;
  const activeGroup = group === null ? null : groupById.get(group) ?? null;

  const validate = useMutation({
    mutationFn: async (ids?: number[]) => ({ started: Date.now(), list: await api.accountsValidate(ids) }),
    onSuccess: ({ started, list }, ids) => {
      qc.setQueryData(qk.accounts, list);
      const asked = ids ? list.filter((a) => ids.includes(a.id)) : list;
      // Accounts Roblox couldn't be asked about (network, rate limit) keep their old stamp.
      const done = asked.filter((a) => (a.statusCheckedAt ?? 0) >= started - 1000);
      const missed = asked.length - done.length;
      const bad = done.filter((a) => a.status === "invalid").length;
      const ch = done.filter((a) => a.status === "challenged").length;
      toast(
        [
          `Checked ${plural(done.length, "account")}`,
          missed && `${missed} couldn't be checked`,
          bad && `${bad} need${bad === 1 ? "s" : ""} sign-in`,
          ch && `${ch} challenged`,
        ]
          .filter(Boolean)
          .join(" · "),
      );
    },
  });
  useEffect(() => {
    if (validatedThisSession || !accounts.length) return;
    validatedThisSession = true;
    validate.mutate(undefined);
  }, []); // eslint-disable-line react-hooks/exhaustive-deps
  const checking = validate.isPending ? (validate.variables ?? null) : undefined;
  const isChecking = (id: number) => checking === null || (checking !== undefined && checking.includes(id));

  const remove = useMutation({
    mutationFn: (ids: number[]) => api.accountsRemove(ids),
    onSuccess: (_, ids) => {
      setAccounts(qc, (list) => list.filter((a) => !ids.includes(a.id)));
      // Ids can be reused by the next account added: drop what belonged to these.
      for (const id of ids) qc.removeQueries({ queryKey: qk.info(id) });
      setChecked(new Set());
      setRemoveOpen(false);
      toast(`Removed ${plural(ids.length, "account")}`);
    },
  });

  const reorder = useMutation({
    mutationFn: (ids: number[]) => api.accountsReorder(ids),
    onError: () => qc.invalidateQueries({ queryKey: qk.accounts }),
  });

  const askRemove = (ids: number[]) => {
    setRemoveIds(ids);
    setRemoveOpen(true);
  };
  const runValidate = () => {
    if (!validate.isPending && accounts.length) validate.mutate(checkedIds.length ? checkedIds : undefined);
  };

  // ---------- reorder (pointer drag on the grip, or arrow keys on it) ----------

  function snapshot() {
    const m = new Map<number, number>();
    for (const r of tbody.current?.rows ?? []) m.set(Number(r.dataset.id), r.getBoundingClientRect().top);
    flip.current = m;
  }

  function applyOrder(ids: number[]) {
    snapshot();
    setOrder(ids);
    setAccounts(qc, (list) => list.map((a) => ({ ...a, sortOrder: ids.indexOf(a.id) })));
    reorder.mutate(ids);
  }

  /** Moves `id` so it sits before shown[over] (or after the last shown row). */
  function dropAt(id: number, over: number) {
    const from = shown.findIndex((a) => a.id === id);
    if (from < 0 || over < 0 || over > shown.length || over === from || over === from + 1) return false;
    const ids = ordered.map((a) => a.id).filter((x) => x !== id);
    const at = over < shown.length ? ids.indexOf(shown[over].id) : ids.indexOf(shown[shown.length - 1].id) + 1;
    ids.splice(at, 0, id);
    applyOrder(ids);
    return true;
  }

  // FLIP: rows glide from where they were to where the new order puts them.
  useLayoutEffect(() => {
    const before = flip.current;
    const rows = tbody.current ? [...tbody.current.rows] : [];
    if (!before || !rows.length) return;
    flip.current = null;
    for (const r of rows) {
      r.style.transition = "none";
      r.style.transform = "";
    }
    const deltas = rows.map((r) => (before.get(Number(r.dataset.id)) ?? r.getBoundingClientRect().top) - r.getBoundingClientRect().top);
    rows.forEach((r, i) => Math.abs(deltas[i]) >= 1 && (r.style.transform = `translateY(${deltas[i]}px)`));
    void tbody.current?.offsetHeight;
    for (const r of rows) {
      r.style.transition = "";
      r.style.transform = "";
    }
  });

  // New rows rise in. Done with WAAPI because a CSS animation restarts whenever React moves the row.
  useLayoutEffect(() => {
    const rows = tbody.current ? [...tbody.current.rows] : [];
    const fresh = rows.filter((r) => !seen.current.has(Number(r.dataset.id)));
    seen.current = new Set(rows.map((r) => Number(r.dataset.id)));
    if (reducedMotion()) return;
    fresh.forEach((r, i) =>
      r.animate([{ opacity: 0, transform: "translateY(8px)" }, { opacity: 1, transform: "none" }], {
        duration: 220,
        easing: "cubic-bezier(.2,.8,.2,1)",
        delay: Math.min(i, 14) * 16,
        fill: "backwards",
      }),
    );
  });

  function gripDown(e: PointerEvent<HTMLButtonElement>, id: number) {
    const row = e.currentTarget.closest("tr");
    if (e.button !== 0 || !row) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    const from = shown.findIndex((a) => a.id === id);
    row.style.transition = "none";
    dragging.current = { id, startY: e.clientY, row, over: from };
    setDrag({ id, over: from });
  }

  function gripMove(e: PointerEvent) {
    const d = dragging.current;
    if (!d || !tbody.current) return;
    d.row.style.transform = `translateY(${e.clientY - d.startY}px)`;
    const rows = [...tbody.current.rows];
    let over = rows.length;
    for (let i = 0; i < rows.length; i++) {
      if (rows[i] === d.row) continue;
      const r = rows[i].getBoundingClientRect();
      if (e.clientY < r.top + r.height / 2) {
        over = i;
        break;
      }
    }
    if (over !== d.over) {
      d.over = over;
      setDrag({ id: d.id, over });
    }
  }

  function gripEnd(cancel: boolean) {
    const d = dragging.current;
    if (!d) return;
    dragging.current = null;
    setDrag(null);
    if (cancel || !dropAt(d.id, d.over)) snapshot(); // glide back home
  }

  function gripKey(e: ReactKeyboardEvent, id: number) {
    const dir = e.key === "ArrowUp" ? -1 : e.key === "ArrowDown" ? 1 : 0;
    if (!dir) return;
    e.preventDefault();
    const i = shown.findIndex((a) => a.id === id);
    if (dropAt(id, dir < 0 ? i - 1 : i + 2)) {
      requestAnimationFrame(() => document.querySelector<HTMLElement>(`[data-grip="${id}"]`)?.focus());
    }
  }

  const from = drag ? shown.findIndex((a) => a.id === drag.id) : -1;
  const dropLine = drag && drag.over !== from && drag.over !== from + 1 ? drag.over : -1;

  // ---------- selection and shortcuts ----------

  const toggle = (id: number) =>
    setChecked((s) => {
      const n = new Set(s);
      if (!n.delete(id)) n.add(id);
      return n;
    });
  const shownChecked = shown.filter((a) => checked.has(a.id)).length;
  const allShown = shown.length > 0 && shownChecked === shown.length;
  const toggleAll = () =>
    setChecked((s) => {
      const n = new Set(s);
      for (const a of shown) allShown ? n.delete(a.id) : n.add(a.id);
      return n;
    });

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (document.querySelector("dialog[open], :popover-open")) return;
      const typing = isTyping(e.target);
      const ctrl = e.ctrlKey || e.metaKey;
      const k = e.key.toLowerCase();
      if (ctrl && k === "a" && !typing) {
        e.preventDefault();
        setChecked((s) => new Set([...s, ...shown.map((a) => a.id)]));
      } else if (ctrl && k === "r") {
        e.preventDefault();
        runValidate();
      } else if (e.key === "Delete" && !typing) {
        const ids = checkedIds.length ? checkedIds : current ? [current.id] : [];
        if (ids.length) askRemove(ids);
      } else if (e.key === "Escape" && !typing) {
        setChecked(new Set());
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  async function addToGroup(g: Group) {
    const targets = checkedIds.flatMap((id) => byId.get(id) ?? []).filter((a) => !a.groupIds.includes(g.id));
    try {
      for (const a of targets) await update.mutateAsync({ id: a.id, patch: { groupIds: [...a.groupIds, g.id] } });
      toast(`Added ${plural(checkedIds.length, "account")} to ${g.name}`);
    } catch {
      // The mutation cache already showed the error.
    }
  }

  const counts = (f: Filter) => matching.filter((a) => inFilter(a, f, presence?.get(a.id))).length;

  return (
    <>
      <main className={`@container flex-1 min-w-0 flex flex-col ${compact ? "compact" : ""}`}>
        <div className="flex flex-wrap items-center gap-3 px-5 pt-[18px] pb-3">
          <h1 className="text-xl font-semibold tracking-[-0.01em]">Accounts</h1>
          <span className="mono text-dim">
            {accounts.length} total
            {needsSignIn > 0 && <> · <span className="text-orange">{needsSignIn} need{needsSignIn === 1 ? "s" : ""} sign-in</span></>}
          </span>
          <div className="ml-auto flex flex-wrap gap-2">
            <button className="btn" onClick={() => !validate.isPending && validate.mutate(undefined)} disabled={validate.isPending || !accounts.length} title="Validate all (Ctrl+R)">
              <Icon name="refresh" size={14} className={validate.isPending ? "spin" : ""} />
              {validate.isPending ? "Validating…" : "Validate all"}
            </button>
            <Segmented
              label="Density"
              value={density}
              options={[{ value: "detailed", label: "Detailed" }, { value: "compact", label: "Compact" }]}
              onChange={(d) => setSettings.mutate({ density: d })}
            />
            <button className="btn btn-primary" onClick={onAdd} title="Add account (Ctrl+N)">
              <Icon name="plus" size={14} />
              Add account
            </button>
          </div>
        </div>

        {accounts.length === 0 ? (
          <Empty onAdd={onAdd} />
        ) : (
          <>
            <div className="flex flex-wrap items-center gap-2 px-5 pb-3.5">
              <label className="flex items-center gap-2 flex-[0_1_260px] h-[30px] px-2.5 border border-line bg-panel text-dim focus-within:border-[var(--focus)]">
                <Icon name="search" size={14} />
                <span className="sr-only">Filter accounts</span>
                <input
                  id="account-filter"
                  value={q}
                  onChange={(e) => setQ(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Escape" && q) {
                      e.stopPropagation();
                      setQ("");
                    }
                  }}
                  placeholder="Filter by name, @user, id"
                  spellCheck={false}
                  className="flex-1 min-w-0 bg-transparent outline-none text-text"
                />
              </label>
              {FILTERS.map((f) => (
                <button key={f.key} className="chip" aria-pressed={filter === f.key} onClick={() => setFilter(f.key)}>
                  {f.key === "fav" && <Icon name="star" size={12} fill={filter === "fav"} />}
                  {f.label}
                  <span className="mono text-[11px] opacity-80">{counts(f.key)}</span>
                </button>
              ))}
              {activeGroup && (
                <button className="chip" aria-pressed="true" onClick={() => setGroup(null)} aria-label={`Clear group filter ${activeGroup.name}`}>
                  <span aria-hidden="true" className="w-2 h-2" style={{ background: groupColor(activeGroup) }} />
                  {activeGroup.name}
                  <Icon name="x" size={12} />
                </button>
              )}
            </div>

            <div className="scroll flex-1 min-h-0 border-t border-line">
              <table className="w-full border-collapse">
                <thead>
                  <tr>
                    <th className="w-[68px] pl-[35px]">
                      <input
                        type="checkbox"
                        className="cb"
                        aria-label="Select all shown"
                        checked={allShown}
                        ref={(el) => {
                          if (el) el.indeterminate = shownChecked > 0 && !allShown;
                        }}
                        onChange={toggleAll}
                      />
                    </th>
                    <th className="label">Account</th>
                    <th className="label w-[34%]">Status</th>
                    <th className="label w-[24%] hidden @2xl:table-cell">Groups</th>
                    <th className="w-12"><span className="sr-only">Favourite</span></th>
                  </tr>
                </thead>
                <tbody ref={tbody}>
                  {shown.map((a, i) => {
                    const name = accountName(a);
                    const cls = [
                      "row",
                      drag?.id === a.id && "row-drag",
                      dropLine === i && "drop-top",
                      dropLine === shown.length && i === shown.length - 1 && "drop-bottom",
                    ];
                    return (
                      <tr
                        key={a.id}
                        data-id={a.id}
                        data-selected={current?.id === a.id || undefined}
                        className={cls.filter(Boolean).join(" ")}
                        onClick={() => setSelected(a.id)}
                      >
                        <td className="pl-3">
                          <div className="flex items-center gap-[5px]">
                            <button
                              type="button"
                              className="grip"
                              data-grip={a.id}
                              aria-label={`Reorder ${name}; arrow keys move it`}
                              onPointerDown={(e) => gripDown(e, a.id)}
                              onPointerMove={gripMove}
                              onPointerUp={() => gripEnd(false)}
                              onPointerCancel={() => gripEnd(true)}
                              onKeyDown={(e) => gripKey(e, a.id)}
                              onClick={(e) => e.stopPropagation()}
                            >
                              <Icon name="grip" size={14} />
                            </button>
                            <input
                              type="checkbox"
                              className="cb"
                              aria-label={`Select ${name}`}
                              checked={checked.has(a.id)}
                              onChange={() => toggle(a.id)}
                              onClick={(e) => e.stopPropagation()}
                            />
                          </div>
                        </td>
                        <td className="pr-3">
                          <div className="flex items-center gap-2.5 min-w-0">
                            <Avatar account={a} size={compact ? 22 : 28} />
                            <div className={compact ? "flex items-baseline gap-2 min-w-0" : "flex flex-col min-w-0 leading-tight"}>
                              <button type="button" className="linkbtn font-semibold truncate" onClick={() => setSelected(a.id)}>
                                {name}
                              </button>
                              <span className="mono text-xs text-dim truncate">@{a.username}</span>
                            </div>
                          </div>
                        </td>
                        <td className="pr-3 max-w-0">
                          <Status
                            status={a.status}
                            presence={presence?.get(a.id)}
                            checking={isChecking(a.id)}
                            detail={!compact && a.statusCheckedAt ? ago(a.statusCheckedAt, now) : undefined}
                          />
                        </td>
                        <td className="pr-3 hidden @2xl:table-cell">
                          <div className="flex flex-wrap gap-1">
                            {a.groupIds.flatMap((gid) => groupById.get(gid) ?? []).map((g) => (
                              <span key={g.id} className="tag">
                                <span aria-hidden="true" className="w-1.5 h-1.5" style={{ background: groupColor(g) }} />
                                {g.name}
                              </span>
                            ))}
                          </div>
                        </td>
                        <td className="pr-2">
                          <button
                            type="button"
                            className="wc w-9 h-9"
                            style={a.favourite ? { color: "var(--accent-text)" } : undefined}
                            aria-label={`Favourite ${name}`}
                            aria-pressed={a.favourite}
                            onClick={(e) => {
                              e.stopPropagation();
                              update.mutate({ id: a.id, patch: { favourite: !a.favourite } });
                            }}
                          >
                            <Icon name="star" size={14} fill={a.favourite} />
                          </button>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
              {shown.length === 0 && (
                <div className="enter flex flex-col items-center gap-3 py-16 text-dim">
                  No accounts match.
                  <button
                    className="btn btn-sm"
                    onClick={() => {
                      setQ("");
                      setFilter("all");
                      setGroup(null);
                    }}
                  >
                    Clear filters
                  </button>
                </div>
              )}
            </div>
          </>
        )}

        {checkedIds.length > 0 && (
          <div className="enter flex flex-wrap items-center gap-2.5 px-5 py-2.5 border-t border-line-strong bg-raised">
            <span className="mono font-medium">{checkedIds.length} selected</span>
            <span className="w-px h-5 bg-line" aria-hidden="true" />
            <button className="btn" onClick={() => validate.mutate(checkedIds)} disabled={validate.isPending}>
              <Icon name="refresh" size={14} />
              Validate
            </button>
            <button className="btn" popoverTarget="bulk-groups" style={{ anchorName: "--bulk-groups" }}>
              Add to group
              <Icon name="chevron" size={12} />
            </button>
            <div id="bulk-groups" popover="auto" className="menu" style={{ positionAnchor: "--bulk-groups" }}>
              {groups.length === 0 && <div className="px-2.5 py-2 text-dim">No groups yet. Create one in the sidebar.</div>}
              {groups.map((g) => (
                <button
                  key={g.id}
                  className="menu-item"
                  popoverTarget="bulk-groups"
                  popoverTargetAction="hide"
                  onClick={() => addToGroup(g)}
                >
                  <span aria-hidden="true" className="w-2 h-2" style={{ background: groupColor(g) }} />
                  {g.name}
                </button>
              ))}
            </div>
            <button className="btn btn-danger" onClick={() => askRemove(checkedIds)}>
              <Icon name="trash" size={14} />
              Remove
            </button>
            <button className="btn ml-auto" onClick={() => setChecked(new Set())}>Clear</button>
          </div>
        )}

        <footer className="mono h-7 flex-none flex items-center gap-4 px-5 border-t border-line text-[11px] text-dim">
          <span>{lastChecked ? `validated ${ago(lastChecked, now)}` : "not validated yet"}</span>
          <span className="ml-auto">v0.1.0</span>
        </footer>
      </main>

      {current && <Divider panel={detail} side="right" label="Details width" />}
      {current && (
        <aside
          key={current.id}
          // Pin the panel to this account once it is used, so an edit that drops it from the
          // filtered list can't swap the panel to another account under the next click.
          onFocusCapture={() => setSelected(current.id)}
          aria-label="Account details"
          style={{ width: detail.width }}
          className="scroll flex-none bg-panel border-l border-line">
          <DetailPanel
            account={current}
            groups={groups}
            vaultMode={vaultMode}
            clipboardSeconds={clipboardSeconds}
            checking={isChecking(current.id)}
            onValidate={() => validate.mutate([current.id])}
            onRemove={() => askRemove([current.id])}
            onLaunch={() => onLaunch(current.id)}
          />
        </aside>
      )}

      <ConfirmDialog
        open={removeOpen}
        onClose={() => setRemoveOpen(false)}
        onConfirm={() => remove.mutate(removeIds)}
        busy={remove.isPending}
        title={removeIds.length === 1 ? "Remove account?" : `Remove ${removeIds.length} accounts?`}
        text={
          <>
            {removeIds.length === 1 ? (
              <>
                <span className="text-text font-medium">{byId.has(removeIds[0]) ? accountName(byId.get(removeIds[0])!) : "This account"}</span> and its saved cookie
              </>
            ) : (
              "These accounts and their saved cookies"
            )}{" "}
            are deleted from this PC. The Roblox accounts themselves are not touched.
          </>
        }
        confirmLabel="Remove"
      />
    </>
  );
}

function Empty({ onAdd }: { onAdd: () => void }) {
  return (
    <div className="enter flex-1 grid place-items-center border-t border-line p-8">
      <div className="flex flex-col items-center gap-4 max-w-sm text-center">
        <span className="w-12 h-12 grid place-content-center border border-line-strong text-dim">
          <Icon name="accounts" size={20} />
        </span>
        <h2 className="text-[15px] font-semibold">No accounts yet</h2>
        <p className="text-dim">
          Sign in through Roblox's own page or paste a .ROBLOSECURITY cookie. Cookies are encrypted and stay on this PC.
        </p>
        <button className="btn btn-primary" onClick={onAdd}>
          <Icon name="plus" size={14} />
          Add account
        </button>
      </div>
    </div>
  );
}
