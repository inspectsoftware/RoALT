import { useState, type FormEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { api, type AccountSummary, type Group } from "../../api";
import { groupColor, qk, useSaveGroup } from "../../data";
import { ConfirmDialog } from "../../ui/Dialog";
import { Icon } from "../../ui/Icon";
import { toast } from "../../ui/Toast";
import { SWATCHES } from "./shared";

/** Sidebar group list: click filters, right-click or the pencil edits, "+" creates. */
export function GroupsNav({ groups, accounts, active, onPick }: {
  groups: Group[];
  accounts: AccountSummary[];
  active: number | null;
  onPick: (id: number | null) => void;
}) {
  const qc = useQueryClient();
  const [editing, setEditing] = useState<number | "new" | null>(null);
  const [deleting, setDeleting] = useState<Group | null>(null);
  const [deleteOpen, setDeleteOpen] = useState(false);

  const remove = useMutation({
    mutationFn: (g: Group) => api.groupRemove(g.id),
    onSuccess: (_, g) => {
      qc.setQueryData<Group[]>(qk.groups, (list) => list?.filter((x) => x.id !== g.id));
      qc.invalidateQueries({ queryKey: qk.accounts });
      if (active === g.id) onPick(null);
      setDeleteOpen(false);
      setEditing(null);
      toast(`Deleted group ${g.name}`);
    },
  });

  return (
    <div className="flex flex-col gap-0.5">
      <div className="flex items-center justify-between pl-3 pt-5 pb-1.5">
        <span className="label">Groups</span>
        <button className="wc w-7 h-7" aria-label="New group" title="New group" onClick={() => setEditing("new")}>
          <Icon name="plus" size={12} />
        </button>
      </div>
      {groups.map((g) =>
        editing === g.id ? (
          <GroupEditor
            key={g.id}
            group={g}
            groups={groups}
            onDone={() => setEditing(null)}
            onDelete={() => {
              setDeleting(g);
              setDeleteOpen(true);
            }}
          />
        ) : (
          <div
            key={g.id}
            className="group relative"
            onContextMenu={(e) => {
              e.preventDefault();
              setEditing(g.id);
            }}
          >
            <button className="nav-item" aria-pressed={active === g.id} onClick={() => onPick(active === g.id ? null : g.id)}>
              <span aria-hidden="true" className="w-2 h-2 flex-none" style={{ background: groupColor(g) }} />
              <span className="flex-1 truncate">{g.name}</span>
              <span className="mono text-xs group-hover:opacity-0 group-focus-within:opacity-0">
                {accounts.filter((a) => a.groupIds.includes(g.id)).length}
              </span>
            </button>
            <button
              className="wc w-8 h-8 absolute right-0.5 top-0.5 opacity-0 group-hover:opacity-100 focus-visible:opacity-100"
              aria-label={`Edit group ${g.name}`}
              title="Rename, recolour or delete"
              onClick={() => setEditing(g.id)}
            >
              <Icon name="pencil" size={12} />
            </button>
          </div>
        ),
      )}
      {editing === "new" && <GroupEditor groups={groups} onDone={() => setEditing(null)} />}
      {groups.length === 0 && editing !== "new" && (
        <button className="sub text-xs" onClick={() => setEditing("new")}>
          Group accounts to filter them in one click.
        </button>
      )}

      <ConfirmDialog
        open={deleteOpen}
        onClose={() => setDeleteOpen(false)}
        onConfirm={() => deleting && remove.mutate(deleting)}
        busy={remove.isPending}
        title="Delete group?"
        text={
          <>
            <span className="text-text font-medium">{deleting?.name}</span> is deleted. Its accounts stay; they just leave the
            group.
          </>
        }
        confirmLabel="Delete group"
      />
    </div>
  );
}

function GroupEditor({ group, groups, onDone, onDelete }: {
  group?: Group;
  groups: Group[];
  onDone: () => void;
  onDelete?: () => void;
}) {
  const save = useSaveGroup();
  const [name, setName] = useState(group?.name ?? "");
  const [color, setColor] = useState(group?.color ?? SWATCHES[groups.length % SWATCHES.length]);
  const [error, setError] = useState<string | null>(null);

  function submit(e: FormEvent) {
    e.preventDefault();
    const n = name.trim();
    if (!n) return onDone();
    if (groups.some((g) => g.id !== group?.id && g.name.toLowerCase() === n.toLowerCase())) {
      return setError("A group with that name exists.");
    }
    save.mutate({ id: group?.id, name: n, color }, { onSuccess: onDone });
  }

  return (
    <form
      onSubmit={submit}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          onDone();
        }
      }}
      className="enter flex flex-col gap-2 p-2 my-0.5 border border-line-strong bg-bg"
    >
      <input
        autoFocus
        className="field h-8"
        aria-label="Group name"
        placeholder="Group name"
        maxLength={32}
        value={name}
        aria-invalid={!!error}
        onChange={(e) => {
          setName(e.target.value);
          setError(null);
        }}
      />
      {error && <span role="alert" className="text-xs text-orange">{error}</span>}
      <div className="flex flex-wrap gap-1.5" role="group" aria-label="Group colour">
        {SWATCHES.map((c) => (
          <button
            key={c}
            type="button"
            className="swatch w-4 h-4"
            style={{ background: c }}
            aria-label={`Colour ${c}`}
            aria-pressed={c === color}
            onClick={() => setColor(c)}
          />
        ))}
      </div>
      <div className="flex gap-1.5">
        <button type="submit" className="btn btn-primary btn-sm" disabled={save.isPending}>
          {group ? "Save" : "Create"}
        </button>
        <button type="button" className="btn btn-sm" onClick={onDone}>Cancel</button>
        {onDelete && (
          <button type="button" className="btn btn-sm btn-danger ml-auto px-2" aria-label="Delete group" title="Delete group" onClick={onDelete}>
            <Icon name="trash" size={12} />
          </button>
        )}
      </div>
    </form>
  );
}
