import { useEffect, useState } from "react";
import { api } from "../../api";
import { setSplashPhase, type SplashPhase } from "./phase";
import "./splash.css";

/** When the window grows: the animation settles at 3 s (79% of 3.8 s), then holds 2 s. */
const GROW_MS = 3000 + 2000;
/** Set once the splash has run: a webview reload doesn't play it again. */
const KEY = "roalt.splash";

/** The AltDock front window and dock bar (`Logo` in ui/Icon); the back window is the cube. */
const TILE =
  "M3 5h7.5a1.5 1.5 0 0 1 1.5 1.5v4.5a1.5 1.5 0 0 1-1.5 1.5h-7.5a1.5 1.5 0 0 1-1.5-1.5v-4.5a1.5 1.5 0 0 1 1.5-1.5z M2.75 14h12.5a1.25 1.25 0 0 1 0 2.5h-12.5a1.25 1.25 0 0 1 0-2.5z";

function Tile({ className, fill }: { className: string; fill: string }) {
  return (
    <svg className={className} width="120" height="120" viewBox="0 0 18 18" aria-hidden="true">
      <path fill={fill} d={TILE} />
    </svg>
  );
}

function seen() {
  // A dev preview with ?splash=1 plays it again.
  if (import.meta.env.DEV && new URLSearchParams(location.search).has("splash")) return false;
  // Settings.skip_splash, from `main_window`: the window is already full size.
  if ((window as { __ROALT_SKIP_SPLASH?: boolean }).__ROALT_SKIP_SPLASH) return true;
  try {
    return sessionStorage.getItem(KEY) !== null;
  } catch {
    return false;
  }
}

// Decided before anything renders, so the island knows to wait for it.
const FIRST: SplashPhase = seen() ? "gone" : "play";
setSplashPhase(FIRST);

/** The start-up splash over the app: the tile glitches into the AltDock mark, then the window
 * (opened small by `splash::start`) grows into the app and this fades away. */
export function Splash() {
  const [state, setState] = useState<SplashPhase>(FIRST);

  useEffect(() => setSplashPhase(state), [state]);

  useEffect(() => {
    if (state !== "play") return;
    // Plays even with Windows' animation effects off (theme.css leaves the splash out).
    const grow = () => {
      try {
        sessionStorage.setItem(KEY, "1");
      } catch {
        // No storage: a reload plays it again.
      }
      // Fades once the window has grown, even if growing failed.
      api.splashDone().finally(() => setState("out"));
    };
    const t = setTimeout(grow, GROW_MS);
    return () => clearTimeout(t);
  }, [state]);

  if (state === "gone") return null;
  return (
    <div
      className={`splash ${state === "out" ? "out" : ""}`}
      data-tauri-drag-region
      onTransitionEnd={(e) => e.target === e.currentTarget && setState("gone")}
      aria-hidden="true"
    >
      <div className="mark" data-tauri-drag-region>
        <div className="logo">
          <div className="cube" />
          <div className="tile">
            <Tile className="red" fill="#ff2a4d" />
            <Tile className="cyan" fill="#22e8ff" />
            <Tile className="core" fill="#949494" />
            <Tile className="slice-a" fill="#ededed" />
            <Tile className="slice-b" fill="#22e8ff" />
          </div>
        </div>
      </div>
      <div className="mark">
        <div className="word">
          <b>ALTDOCK</b>
          <span>v{import.meta.env.APP_VERSION}</span>
        </div>
      </div>
      <div className="scan" />
    </div>
  );
}
