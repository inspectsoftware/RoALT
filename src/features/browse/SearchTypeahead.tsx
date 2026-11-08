import "./SearchTypeahead.css";
import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { api, asError, type GameSearchResult, type UserSearchResult } from "../../api";
import { Input } from "../../ui/Field";
import { t } from "../../i18n";

/** Browse's People / Games search typeahead. Wraps RoALT's `<Input>` and, as the user types,
 * drops a list of up to 5 best Roblox matches + thumbnails below it. Click a row to open that
 * profile / game inside the Browse pane. Submitting the form (Enter) still runs the full search
 * the normal way, which the parent owns. */
type NativeOverlay = {
  rect: { left: number; top: number; width: number };
  rows: { title: string; subtitle: string; thumbnail?: string | null; square: boolean; url: string }[];
  note?: string;
};

export function SearchTypeahead({ kind, query, setQuery, onPickUser, onPickGame, nativeOverlay, onNativeOverlay }: {
  kind: "people" | "games";
  query: string;
  setQuery: (q: string) => void;
  onPickUser: (userId: number) => void;
  onPickGame: (placeId: number) => void;
  /** When Browse's native Roblox page is visible, suggestions render inside that page. */
  nativeOverlay?: boolean;
  onNativeOverlay?: (overlay: NativeOverlay | null) => void;
}) {
  const [open, setOpen] = useState(false);
  const [debounced, setDebounced] = useState("");
  const wrap = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const id = window.setTimeout(() => setDebounced(query.trim()), 180);
    return () => window.clearTimeout(id);
  }, [query]);

  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => {
      if (!wrap.current?.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", away);
    return () => document.removeEventListener("mousedown", away);
  }, [open]);

  const needle = debounced.length >= 2 ? debounced : "";
  const people = useQuery({
    queryKey: ["browseTypeahead", "people", needle],
    queryFn: () => api.searchUsers(needle),
    enabled: kind === "people" && needle.length > 0,
    staleTime: 60_000,
    retry: false,
  });
  const games = useQuery({
    queryKey: ["browseTypeahead", "games", needle],
    queryFn: () => api.searchGames(needle),
    enabled: kind === "games" && needle.length > 0,
    staleTime: 60_000,
    retry: false,
  });
  const active = kind === "people" ? people : games;
  const show = open && needle.length > 0;

  useEffect(() => {
    if (!nativeOverlay || !show || !wrap.current) {
      onNativeOverlay?.(null);
      return;
    }
    const r = wrap.current.getBoundingClientRect();
    const rows = kind === "people"
      ? (people.data ?? []).map((u) => ({ title: u.displayName || u.username, subtitle: `@${u.username}`, thumbnail: u.thumbnail, square: false, url: `https://www.roblox.com/users/${u.userId}/profile` }))
      : (games.data ?? []).map((g) => ({ title: g.name || `#${g.placeId}`, subtitle: `${g.creatorName || ""}${g.playerCount != null && g.playerCount > 0 ? ` · ${g.playerCount.toLocaleString()}` : ""}`, thumbnail: g.thumbnail, square: true, url: `https://www.roblox.com/games/${g.placeId}` }));
    const note = active.isFetching && !active.data ? t("common.loading") : active.isError ? asError(active.error).message || t("common.error") : !rows.length ? t(kind === "people" ? "headerSearch.noPeople" : "headerSearch.noGames") : undefined;
    onNativeOverlay?.({ rect: { left: r.left, top: r.bottom + 6, width: r.width }, rows, note });
    return () => onNativeOverlay?.(null);
  }, [active.data, active.error, active.isError, active.isFetching, kind, nativeOverlay, onNativeOverlay, people.data, show]);

  const pickUser = (u: UserSearchResult) => { setQuery(""); setOpen(false); onPickUser(u.userId); };
  const pickGame = (g: GameSearchResult) => { setQuery(""); setOpen(false); onPickGame(g.placeId); };

  return (
    <div ref={wrap} className="browse-typeahead">
      <Input
        data-find
        leading="search"
        aria-label={t("browse.search")}
        placeholder={t("browse.search")}
        value={query}
        onFocus={() => setOpen(true)}
        onChange={(e) => { setQuery(e.target.value); setOpen(true); }}
        onKeyDown={(e) => { if (e.key === "Escape") { setOpen(false); (e.currentTarget as HTMLInputElement).blur(); } }}
        aria-expanded={show}
      />
      {show && !nativeOverlay && (
        <div role="listbox" className="browse-typeahead-popover">
          {active.isFetching && !active.data ? (
            <div aria-busy="true" className="browse-typeahead-note">{t("common.loading")}</div>
          ) : active.isError ? (
            <div role="alert" className="browse-typeahead-error">{asError(active.error).message || t("common.error")}</div>
          ) : kind === "people" ? (
            (people.data ?? []).length === 0 ? (
              <div className="browse-typeahead-note">{t("headerSearch.noPeople")}</div>
            ) : (
              (people.data ?? []).map((u) => (
                <button key={u.userId} type="button" role="option" className="browse-typeahead-row" onClick={() => pickUser(u)}>
                  <Thumb src={u.thumbnail} alt="" />
                  <span className="browse-typeahead-text">
                    <span className="truncate text-sm">{u.displayName || u.username}</span>
                    <span className="truncate text-xs text-fg-3 font-mono">@{u.username}</span>
                  </span>
                </button>
              ))
            )
          ) : (
            (games.data ?? []).length === 0 ? (
              <div className="browse-typeahead-note">{t("headerSearch.noGames")}</div>
            ) : (
              (games.data ?? []).map((g) => (
                <button key={g.placeId} type="button" role="option" className="browse-typeahead-row" onClick={() => pickGame(g)}>
                  <Thumb src={g.thumbnail} alt="" square />
                  <span className="browse-typeahead-text">
                    <span className="truncate text-sm">{g.name || `#${g.placeId}`}</span>
                    <span className="truncate text-xs text-fg-3">
                      {g.creatorName || " "}
                      {g.playerCount != null && g.playerCount > 0 ? ` · ${g.playerCount.toLocaleString()}` : ""}
                    </span>
                  </span>
                </button>
              ))
            )
          )}
        </div>
      )}
    </div>
  );
}

function Thumb({ src, alt, square }: { src?: string | null; alt: string; square?: boolean }) {
  const cls = square ? "browse-typeahead-thumb browse-typeahead-thumb-square" : "browse-typeahead-thumb browse-typeahead-thumb-round";
  if (!src) return <span aria-hidden="true" className={cls} />;
  return <img src={src} alt={alt} className={cls} loading="lazy" />;
}
