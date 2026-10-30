import { useEffect, useMemo, useRef, useState } from "react";
import { save as saveFile } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { api, events } from "../../api";
import { Icon } from "../../ui/Icon";
import { toast, toastError } from "../../ui/Toast";
import { t } from "../../i18n";

/** Lines kept in view; older ones stay in roalt.log. */
const KEEP = 5000;
/** History read from roalt.log when the tab opens. */
const HISTORY = 2000;

const LEVELS = ["error", "warn", "info", "debug"] as const;
type Level = (typeof LEVELS)[number];
type Line = { id: number; level: Level; text: string };

/** tauri-plugin-log's numeric levels (Trace 1 … Error 5). */
const BY_NUMBER: Record<number, Level> = { 1: "debug", 2: "debug", 3: "info", 4: "warn", 5: "error" };

/** The `[LEVEL]` tag of a roalt.log line; lines without one (a panic's next lines) follow `prev`. */
function levelOf(text: string, prev: Level): Level {
  const m = /\[(TRACE|DEBUG|INFO|WARN|ERROR)\]/.exec(text);
  if (!m) return text.startsWith("[fatal]") ? "error" : prev;
  return m[1] === "TRACE" ? "debug" : (m[1].toLowerCase() as Level);
}

const COLOR: Record<Level, string> = { error: "text-err", warn: "text-warn", info: "text-text", debug: "text-dim" };

let nextId = 0;

/** Everything RoALT logs (roalt.log), live, terminal-style. */
export function DebugView() {
  const [lines, setLines] = useState<Line[]>([]);
  const [shown, setShown] = useState<Record<Level, boolean>>({ error: true, warn: true, info: true, debug: false });
  const [query, setQuery] = useState("");
  const [paused, setPaused] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  // Lines that arrived while paused, added on resume.
  const held = useRef<Line[]>([]);
  const pausedRef = useRef(paused);
  pausedRef.current = paused;

  useEffect(() => {
    let live = true;
    const add = (more: Line[]) => setLines((l) => [...l, ...more].slice(-KEEP));
    // Live lines first, so none are lost while the history loads.
    const early: Line[] = [];
    let loaded = false;
    const un = events.onLog((p) => {
      const line = { id: nextId++, level: BY_NUMBER[p.level] ?? levelOf(p.message, "info"), text: p.message };
      if (!loaded) early.push(line);
      else if (pausedRef.current) held.current.push(line);
      else add([line]);
    });
    api
      .debugTail(HISTORY)
      .then((text) => {
        if (!live) return;
        let prev: Level = "info";
        const history = text.map((s) => {
          prev = levelOf(s, prev);
          return { id: nextId++, level: prev, text: s };
        });
        setLines([...history, ...early].slice(-KEEP));
      })
      .catch(toastError)
      .finally(() => {
        loaded = true;
      });
    return () => {
      live = false;
      un.then((f) => f());
    };
  }, []);

  useEffect(() => {
    if (!paused && held.current.length) {
      const more = held.current;
      held.current = [];
      setLines((l) => [...l, ...more].slice(-KEEP));
    }
  }, [paused]);

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    return lines.filter((l) => shown[l.level] && (!q || l.text.toLowerCase().includes(q)));
  }, [lines, shown, query]);

  const counts = useMemo(() => {
    const c: Record<Level, number> = { error: 0, warn: 0, info: 0, debug: 0 };
    for (const l of lines) c[l.level]++;
    return c;
  }, [lines]);

  // Follows the newest line unless paused.
  useEffect(() => {
    if (!paused && box.current) box.current.scrollTop = box.current.scrollHeight;
  }, [visible, paused]);

  const copy = () =>
    writeText(visible.map((l) => l.text).join("\n")).then(() => toast(t("debug.copied")), toastError);
  const copyReport = () =>
    api
      .diagnosticsText()
      .then(writeText)
      .then(() => toast(t("debug.reportCopied")), toastError);
  const saveReport = async () => {
    try {
      const path = await saveFile({ defaultPath: `RoALT-diagnostics-${new Date().toISOString().slice(0, 10)}.txt`, filters: [{ name: t("debug.reportFilter"), extensions: ["txt"] }] });
      if (!path) return;
      await api.diagnosticsSave(path);
      toast(t("debug.reportSaved"));
    } catch (e) {
      toastError(e);
    }
  };

  return (
    <main className="flex-1 min-w-0 flex flex-col px-10 pt-6 pb-6 min-h-0">
      <h1 className="text-xl font-semibold tracking-[-0.01em]">{t("debug.title")}</h1>
      <p className="text-dim mt-1">{t("debug.intro")}</p>

      <div className="flex flex-wrap items-center gap-2 mt-4">
        {LEVELS.map((lv) => (
          <button
            key={lv}
            className="btn btn-sm mono"
            aria-pressed={shown[lv]}
            style={shown[lv] ? { background: "var(--sel)" } : { opacity: 0.6 }}
            onClick={() => setShown((s) => ({ ...s, [lv]: !s[lv] }))}
          >
            <span className={COLOR[lv]}>{t(`debug.level.${lv}`)}</span>
            <span className="text-dim">{counts[lv]}</span>
          </button>
        ))}
        <input
          className="field h-7 px-2 flex-1 min-w-[160px]"
          type="search"
          placeholder={t("debug.filter")}
          aria-label={t("debug.filter")}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        <button className="btn btn-sm" aria-pressed={paused} onClick={() => setPaused((p) => !p)}>
          {paused ? t("debug.resume") : t("debug.pause")}
        </button>
        <button className="btn btn-sm" onClick={copy} disabled={!visible.length}>
          <Icon name="copy" />
          {t("debug.copy")}
        </button>
        <button className="btn btn-sm" onClick={() => setLines([])}>
          <Icon name="trash" />
          {t("debug.clear")}
        </button>
        <button className="btn btn-sm" onClick={() => api.logsOpen().catch(toastError)}>
          <Icon name="external" />
          {t("debug.openFolder")}
        </button>
        <button className="btn btn-sm" title={t("debug.reportHint")} onClick={copyReport}>
          <Icon name="copy" />
          {t("debug.copyReport")}
        </button>
        <button className="btn btn-sm" title={t("debug.reportHint")} onClick={saveReport}>
          <Icon name="download" />
          {t("debug.saveReport")}
        </button>
      </div>

      <div
        ref={box}
        role="log"
        aria-label={t("debug.title")}
        className="selectable mono scroll flex-1 min-h-0 mt-3 p-3 border border-line bg-panel text-xs leading-5"
        style={{ scrollBehavior: "auto" }}
      >
        {visible.length === 0 ? (
          <p className="text-dim">{t("debug.empty")}</p>
        ) : (
          visible.map((l) => (
            <div key={l.id} className={`whitespace-pre-wrap break-all ${COLOR[l.level]}`}>
              {l.text}
            </div>
          ))
        )}
      </div>
    </main>
  );
}
