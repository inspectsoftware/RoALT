import { useEffect, useRef, useState, type FormEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api, asError, type AccountSummary, type Favourite, type LaunchTarget } from "../../api";
import { qk, useFavourites } from "../../data";
import { Switch } from "../../ui/controls";
import { Dialog } from "../../ui/Dialog";
import { Icon } from "../../ui/Icon";
import { friendly, toast } from "../../ui/Toast";
import { accountName, Avatar, Status } from "../accounts/shared";

/** Sidebar list: click fills the Launch view; right-click (or the pencil) renames or deletes. */
export function FavouritesNav({ accounts, active, onPick }: {
  accounts: AccountSummary[];
  active: number | null;
  onPick: (f: Favourite) => void;
}) {
  const qc = useQueryClient();
  const favs = useFavourites();
  const menu = useRef<HTMLDivElement>(null);
  const [menuFor, setMenuFor] = useState<Favourite | null>(null);
  const [editing, setEditing] = useState<Favourite | null>(null);
  const [editOpen, setEditOpen] = useState(false);

  const remove = useMutation({
    mutationFn: (f: Favourite) => api.favouriteRemove(f.id),
    onSuccess: (_, f) => {
      qc.setQueryData<Favourite[]>(qk.favourites, (list) => list?.filter((x) => x.id !== f.id));
      toast(`Deleted ${f.name}`);
    },
  });

  const openMenu = (f: Favourite) => {
    setMenuFor(f);
    menu.current?.showPopover();
  };

  return (
    <div className="flex flex-col gap-0.5">
      <div className="flex items-center h-7 pl-3 mt-5 mb-1.5">
        <span className="label">Favourites</span>
      </div>
      {favs.isError && <span className="px-3 text-xs text-dim">Couldn't load favourites.</span>}
      {favs.data?.length === 0 && <span className="px-3 text-xs text-dim">Star a target in Launch to keep it here.</span>}
      {favs.data?.map((f) => (
        <div
          key={f.id}
          className="group relative"
          style={{ anchorName: `--fav-${f.id}` }}
          onContextMenu={(e) => {
            e.preventDefault();
            openMenu(f);
          }}
        >
          <button className="nav-item" aria-pressed={active === f.id} onClick={() => onPick(f)}>
            <Icon name="star" size={12} fill />
            <span className="flex-1 truncate" title={f.name}>{f.name}</span>
          </button>
          <button
            className="wc w-8 h-8 absolute right-0.5 top-0.5 opacity-0 group-hover:opacity-100 focus-visible:opacity-100"
            aria-label={`Rename or delete ${f.name}`}
            title="Rename or delete"
            onClick={() => openMenu(f)}
          >
            <Icon name="pencil" size={12} />
          </button>
        </div>
      ))}

      <div ref={menu} id="fav-menu" popover="auto" className="menu" style={{ positionAnchor: menuFor ? `--fav-${menuFor.id}` : undefined }}>
        <button
          className="menu-item"
          popoverTarget="fav-menu"
          popoverTargetAction="hide"
          onClick={() => {
            setEditing(menuFor);
            setEditOpen(true);
          }}
        >
          <Icon name="pencil" size={12} />
          Rename…
        </button>
        <button
          className="menu-item text-orange"
          popoverTarget="fav-menu"
          popoverTargetAction="hide"
          disabled={remove.isPending}
          onClick={() => menuFor && remove.mutate(menuFor)}
        >
          <Icon name="trash" size={12} />
          Delete
        </button>
      </div>

      {editing && (
        <FavouriteDialog
          open={editOpen}
          onClose={() => setEditOpen(false)}
          accounts={accounts}
          initial={editing}
          resolve={() => Promise.resolve(editing.target)}
        />
      )}
    </div>
  );
}

/** Saves a new favourite (no `initial.id`) or edits one; the target comes from `resolve`. */
export function FavouriteDialog({ open, onClose, accounts, initial, resolve, onSaved }: {
  open: boolean;
  onClose: () => void;
  accounts: AccountSummary[];
  initial: { id?: number; name: string; accountIds: number[]; inTray: boolean };
  resolve: () => Promise<LaunchTarget>;
  onSaved?: (f: Favourite) => void;
}) {
  const qc = useQueryClient();
  const [name, setName] = useState("");
  const [ids, setIds] = useState<number[]>([]);
  const [tray, setTray] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (!open) return;
    setName(initial.name);
    setIds(initial.accountIds);
    setTray(initial.inTray);
    setError(null);
  }, [open]); // eslint-disable-line react-hooks/exhaustive-deps

  const toggle = (id: number) => setIds((s) => (s.includes(id) ? s.filter((x) => x !== id) : [...s, id]));

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (busy) return;
    const n = name.trim();
    if (!n) return setError("Give it a name.");
    setBusy(true);
    try {
      const target = await resolve();
      const accountIds = accounts.filter((a) => ids.includes(a.id)).map((a) => a.id);
      const f = await api.favouriteSave({ id: initial.id, name: n, target, accountIds, inTray: tray });
      qc.invalidateQueries({ queryKey: qk.favourites });
      toast(initial.id ? `Saved ${f.name}` : `Added ${f.name} to favourites`);
      onSaved?.(f);
      onClose();
    } catch (err) {
      setError(friendly(asError(err)));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onClose={() => !busy && onClose()} title={initial.id ? "Edit favourite" : "Save as favourite"} width={440}>
      <form onSubmit={submit} className="flex flex-col gap-4 p-5">
        <label className="flex flex-col gap-1.5">
          <span className="label">Name</span>
          <input
            className="field"
            maxLength={64}
            value={name}
            aria-invalid={!!error && !name.trim()}
            onChange={(e) => {
              setName(e.target.value);
              setError(null);
            }}
          />
        </label>
        <div className="flex flex-col gap-1.5">
          <span className="label" id="fav-accounts-label">Default accounts</span>
          <div role="group" aria-labelledby="fav-accounts-label" className="scroll max-h-[208px] border border-line">
            {accounts.length === 0 && <p className="p-3 text-dim">No accounts yet.</p>}
            {accounts.map((a) => (
              <label key={a.id} className="flex items-center gap-2.5 h-[38px] px-3 border-b border-line last:border-b-0 cursor-pointer hover:bg-hover has-[:checked]:bg-sel">
                <input type="checkbox" className="cb" checked={ids.includes(a.id)} onChange={() => toggle(a.id)} />
                <Avatar account={a} size={20} />
                <span className="flex-1 min-w-0 truncate">{accountName(a)}</span>
                <span className="flex justify-end min-w-0 max-w-[45%] text-xs">
                  <Status status={a.status} />
                </span>
              </label>
            ))}
          </div>
          <span className="text-xs text-dim">Picking the favourite selects the first valid one.</span>
        </div>
        <div className="flex items-center justify-between gap-2">
          <span>Show in tray</span>
          <Switch label="Show in tray" checked={tray} onChange={setTray} />
        </div>
        {error && <div role="alert" className="alert">{error}</div>}
        <div className="flex justify-end gap-2">
          <button type="button" className="btn" onClick={onClose} disabled={busy}>Cancel</button>
          <button type="submit" className="btn btn-primary" disabled={busy}>
            <Icon name="star" size={12} fill />
            {initial.id ? "Save" : "Save favourite"}
          </button>
        </div>
      </form>
    </Dialog>
  );
}
