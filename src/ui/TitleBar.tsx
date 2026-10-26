import { useEffect, useMemo, useState } from "react";
import { useIsFetching } from "@tanstack/react-query";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Icon, Logo } from "./Icon";

/** Custom title bar (window decorations are off). Empty space drags; double-click maximizes. */
export function TitleBar({ dark, onToggleTheme, onLock, onRefresh }: {
  dark: boolean;
  onToggleTheme?: () => void;
  onLock?: () => void;
  onRefresh?: () => void;
}) {
  const win = useMemo(() => getCurrentWindow(), []);
  const fetching = useIsFetching() > 0;
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    const sync = () => win.isMaximized().then(setMaximized, () => {});
    sync();
    const un = win.onResized(sync);
    return () => void un.then((f) => f());
  }, [win]);

  return (
    <header data-tauri-drag-region="deep" className="h-10 flex-none flex items-center gap-2.5 pl-3.5 border-b border-line bg-panel">
      <Logo />
      <span className="font-bold tracking-[0.02em]">RoALT</span>
      <div className="ml-auto flex items-center">
        {onRefresh && (
          <button className="wc" aria-label="Refresh" title="Refresh (F5)" onClick={onRefresh}>
            <Icon name="refresh" className={fetching ? "spin" : ""} />
          </button>
        )}
        {onToggleTheme && (
          <button className="wc" aria-label={dark ? "Switch to light theme" : "Switch to dark theme"} title="Theme" onClick={onToggleTheme}>
            <span key={dark ? "d" : "l"} className="pop grid">
              <Icon name={dark ? "sun" : "moon"} />
            </span>
          </button>
        )}
        {onLock && (
          <button className="wc" aria-label="Lock vault" title="Lock vault (Ctrl+L)" onClick={onLock}>
            <Icon name="lock" />
          </button>
        )}
        <span className="w-px h-5 bg-line mx-1" aria-hidden="true" />
        <button className="wc" aria-label="Minimize window" onClick={() => win.minimize()}>
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0 5h10" stroke="currentColor" /></svg>
        </button>
        <button className="wc" aria-label={maximized ? "Restore window" : "Maximize window"} onClick={() => win.toggleMaximize()}>
          {maximized ? (
            <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" aria-hidden="true">
              <rect x="0.5" y="2.5" width="7" height="7" /><path d="M2.5 2.5V0.5h7v7h-2" />
            </svg>
          ) : (
            <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
              <rect x="0.5" y="0.5" width="9" height="9" fill="none" stroke="currentColor" />
            </svg>
          )}
        </button>
        <button className="wc wc-close" aria-label="Close window" onClick={() => win.close()}>
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true"><path d="M0 0l10 10M10 0L0 10" stroke="currentColor" /></svg>
        </button>
      </div>
    </header>
  );
}
