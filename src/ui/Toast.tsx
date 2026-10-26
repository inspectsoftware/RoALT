import { useSyncExternalStore } from "react";
import { asError, type ErrorPayload } from "../api";
import { Icon } from "./Icon";

type Tone = "ok" | "error";
type Item = { id: number; text: string; tone: Tone; leaving?: boolean };

let items: Item[] = [];
let nextId = 0;
const subs = new Set<() => void>();
const publish = (next: Item[]) => {
  items = next;
  subs.forEach((f) => f());
};
const subscribe = (f: () => void) => {
  subs.add(f);
  return () => subs.delete(f);
};

function dismiss(id: number) {
  publish(items.map((t) => (t.id === id ? { ...t, leaving: true } : t)));
  setTimeout(() => publish(items.filter((t) => t.id !== id)), 200);
}

/** Shows a short message bottom-right. Callable from anywhere, including outside React. A sticky
 *  one stays until dismissed. */
export function toast(text: string, tone: Tone = "ok", sticky = false) {
  const id = ++nextId;
  publish([...items.slice(-3), { id, text, tone }]);
  if (!sticky) setTimeout(() => dismiss(id), tone === "error" ? 7000 : 4000);
}

const FRIENDLY: Record<string, string> = {
  locked: "The vault is locked. Unlock it and try again.",
  wrongPassword: "Wrong password.",
  network: "Couldn't reach Roblox. Check your connection and try again.",
  notFound: "That account or group no longer exists.",
  crypto: "Couldn't decrypt the data. The password is wrong or the file is damaged.",
  storage: "Couldn't save to RoALT's local database.",
};

/** User-facing text for an IPC error. Backend messages are already safe and user-facing. */
export function friendly(e: ErrorPayload): string {
  return FRIENDLY[e.kind] ?? (e.message || "Something went wrong.");
}

/** Error toast; a cancelled action (closed window, dismissed Windows Hello) stays quiet. */
export function toastError(e: unknown) {
  const err = asError(e);
  if (err.kind !== "cancelled") toast(friendly(err), "error");
}

export function Toaster() {
  const list = useSyncExternalStore(subscribe, () => items);
  return (
    <div className="fixed right-4 bottom-4 z-50 flex flex-col items-end gap-2 pointer-events-none" aria-live="polite">
      {list.map((t) => (
        <div
          key={t.id}
          role={t.tone === "error" ? "alert" : "status"}
          className={`toast enter ${t.tone === "error" ? "toast-error" : ""} ${t.leaving ? "toast-leave" : ""}`}
        >
          <span className="flex-1 min-w-0 pt-0.5 break-words">{t.text}</span>
          <button className="wc w-6 h-6" aria-label="Dismiss" onClick={() => dismiss(t.id)}>
            <Icon name="x" size={12} />
          </button>
        </div>
      ))}
    </div>
  );
}
