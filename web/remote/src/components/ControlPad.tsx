import { useEffect, useRef, useState } from "react";
import type { RemoteManager } from "../transport";
import { LivePreview } from "./LivePreview";

const HELD_REFRESH_MS = 250;

/** Calls `leave` when the page loses focus or is hidden: the release of a finger or button
 * may then never arrive, and held input must not be refreshed after it. */
export function onPageLeave(leave: () => void): () => void {
  const hidden = () => {
    if (document.visibilityState === "hidden") leave();
  };
  window.addEventListener("blur", leave);
  window.addEventListener("pagehide", leave);
  document.addEventListener("visibilitychange", hidden);
  return () => {
    window.removeEventListener("blur", leave);
    window.removeEventListener("pagehide", leave);
    document.removeEventListener("visibilitychange", hidden);
  };
}

/** Live view you play by touch, as on a phone: a tap on the picture is a real touch on the PC
 * (`inputTouch`, Windows makes it a click), a drag turns the camera (`inputCamera`, the right mouse
 * button held, as Roblox on a PC turns it), and since Roblox on a PC never shows its own thumbstick
 * this draws one: the stick holds W/A/S/D and the jump button Space (`inputKey`). The PC lets go of
 * anything still down when this closes or the connection drops. */
export function ControlPad({
  manager,
  pcId,
  instanceId,
  live,
  emptyText,
  onError,
  onFps,
}: {
  manager: RemoteManager;
  pcId: string;
  instanceId: string;
  live: boolean;
  emptyText: string;
  onError: (msg: string) => void;
  onFps?: (fps: number) => void;
}) {
  /** Fingers on the picture not yet known to be a tap or a drag: where they landed. */
  const pending = useRef(new Map<number, { x: number; y: number; cx: number; cy: number }>());
  /** The finger turning the camera, if any (one at a time). */
  const cameraFinger = useRef<number | null>(null);
  const cameraAt = useRef({ x: 0, y: 0 });
  const lastMove = useRef(0);
  const lastError = useRef(0);
  const keys = useRef(new Set<string>());
  const stickFinger = useRef<number | null>(null);
  const jumpFinger = useRef<number | null>(null);
  const [knob, setKnob] = useState<{ x: number; y: number } | null>(null);

  const fail = (e: unknown) => {
    // One message at a time: a drag or a held key would otherwise repeat the same error.
    if (Date.now() - lastError.current > 3000) {
      lastError.current = Date.now();
      onError(e instanceof Error ? e.message : String(e));
    }
  };
  const rpc = (cmd: Record<string, unknown>) => manager.rpc(pcId, { id: instanceId, ...cmd }).catch(fail);

  const key = (code: string, isDown: boolean) => {
    if (isDown === keys.current.has(code)) return;
    if (isDown) keys.current.add(code);
    else keys.current.delete(code);
    void rpc({ op: "inputKey", code, down: isDown });
  };

  /** The stick at (dx, dy), each -1..1 from its centre: past a third of the way a direction's key is held. */
  const steer = (dx: number, dy: number) => {
    const want = new Set<string>();
    if (dy < -0.35) want.add("KeyW");
    if (dy > 0.35) want.add("KeyS");
    if (dx < -0.35) want.add("KeyA");
    if (dx > 0.35) want.add("KeyD");
    for (const code of ["KeyW", "KeyA", "KeyS", "KeyD"]) key(code, want.has(code));
  };

  const stick = (e: React.PointerEvent<HTMLDivElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    let dx = (e.clientX - (r.left + r.width / 2)) / (r.width / 2);
    let dy = (e.clientY - (r.top + r.height / 2)) / (r.height / 2);
    const len = Math.hypot(dx, dy);
    if (len > 1) {
      dx /= len;
      dy /= len;
    }
    setKnob({ x: dx, y: dy });
    steer(dx, dy);
  };
  const stickOff = (e: React.PointerEvent<HTMLDivElement>) => {
    if (stickFinger.current !== e.pointerId) return;
    stickFinger.current = null;
    setKnob(null);
    steer(0, 0);
  };
  const jumpOff = (e: React.PointerEvent<HTMLButtonElement>) => {
    if (jumpFinger.current !== e.pointerId) return;
    jumpFinger.current = null;
    key("Space", false);
  };

  useEffect(() => {
    const t = window.setInterval(() => {
      const send = (cmd: Record<string, unknown>) => void manager.rpc(pcId, { id: instanceId, ...cmd }).catch(() => {});
      for (const code of keys.current) send({ op: "inputKey", code, down: true, repeat: true });
      if (cameraFinger.current !== null) send({ op: "inputCamera", what: "move", ...cameraAt.current });
    }, HELD_REFRESH_MS);
    return () => window.clearInterval(t);
  }, [manager, pcId, instanceId]);

  const letGo = () => {
    if (cameraFinger.current !== null) void manager.rpc(pcId, { op: "inputCamera", id: instanceId, what: "up", ...cameraAt.current }).catch(() => {});
    for (const code of keys.current) void manager.rpc(pcId, { op: "inputKey", id: instanceId, code, down: false }).catch(() => {});
    cameraFinger.current = null;
    stickFinger.current = null;
    jumpFinger.current = null;
    pending.current.clear();
    keys.current.clear();
    setKnob(null);
  };

  // A camera drag and keys still down when the view closes, or the controls go away with the live picture, are let go.
  useEffect(() => {
    const stopWatching = onPageLeave(letGo);
    return () => {
      stopWatching();
      letGo();
    };
  }, [manager, pcId, instanceId]);
  useEffect(() => {
    if (!live) letGo();
  }, [live]);

  const at = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    return { x: (e.clientX - r.left) / r.width, y: (e.clientY - r.top) / r.height };
  };
  /** A finger leaves the picture: the end of its drag, or, if it barely moved, a tap where it landed. */
  const lift = (e: React.PointerEvent<HTMLCanvasElement>, cancelled: boolean) => {
    if (cameraFinger.current === e.pointerId) {
      cameraFinger.current = null;
      cameraAt.current = at(e);
      void rpc({ op: "inputCamera", what: "move", ...cameraAt.current });
      void rpc({ op: "inputCamera", what: "up", ...cameraAt.current });
      return;
    }
    const p = pending.current.get(e.pointerId);
    pending.current.delete(e.pointerId);
    if (p && !cancelled) {
      void rpc({ op: "inputTouch", touch: 0, what: "down", x: p.x, y: p.y });
      void rpc({ op: "inputTouch", touch: 0, what: "up", x: p.x, y: p.y });
    }
  };

  return live ? (
    <div className="relative w-full bg-black">
      <LivePreview
        manager={manager}
        pcId={pcId}
        instanceId={instanceId}
        onFps={onFps}
        aria-label="Live view"
        className="block h-auto w-full touch-none select-none"
        onContextMenu={(e) => e.preventDefault()}
        onPointerDown={(e) => {
          e.preventDefault();
          e.currentTarget.setPointerCapture(e.pointerId);
          pending.current.set(e.pointerId, { ...at(e), cx: e.clientX, cy: e.clientY });
        }}
        onPointerMove={(e) => {
          const p = pending.current.get(e.pointerId);
          // Past 10 px a finger is a drag: it turns the camera, unless another finger already does.
          if (p && cameraFinger.current === null && Math.hypot(e.clientX - p.cx, e.clientY - p.cy) > 10) {
            pending.current.delete(e.pointerId);
            cameraFinger.current = e.pointerId;
            lastMove.current = Date.now();
            cameraAt.current = at(e);
            void rpc({ op: "inputCamera", what: "down", x: p.x, y: p.y });
            void rpc({ op: "inputCamera", what: "move", ...cameraAt.current });
            return;
          }
          // ~20 moves a second is plenty over the relay.
          if (cameraFinger.current !== e.pointerId || Date.now() - lastMove.current < 50) return;
          lastMove.current = Date.now();
          cameraAt.current = at(e);
          void rpc({ op: "inputCamera", what: "move", ...cameraAt.current });
        }}
        onPointerUp={(e) => lift(e, false)}
        onPointerCancel={(e) => lift(e, true)}
        onLostPointerCapture={(e) => lift(e, true)}
      />
      {/* Roblox-style thumbstick, bottom left over the picture. */}
      <div
        className="absolute bottom-3 left-3 h-24 w-24 touch-none select-none rounded-full border-2 border-white/40 bg-black/25"
        onContextMenu={(e) => e.preventDefault()}
        onPointerDown={(e) => {
          e.preventDefault();
          if (stickFinger.current !== null) return;
          stickFinger.current = e.pointerId;
          e.currentTarget.setPointerCapture(e.pointerId);
          stick(e);
        }}
        onPointerMove={(e) => {
          if (stickFinger.current === e.pointerId) stick(e);
        }}
        onPointerUp={stickOff}
        onPointerCancel={stickOff}
        onLostPointerCapture={stickOff}
      >
        <div
          className="pointer-events-none absolute left-1/2 top-1/2 h-10 w-10 rounded-full bg-white/70"
          style={{ transform: `translate(calc(-50% + ${(knob?.x ?? 0) * 28}px), calc(-50% + ${(knob?.y ?? 0) * 28}px))` }}
        />
      </div>
      {/* Jump, bottom right. */}
      <button
        type="button"
        aria-label="Jump"
        className="absolute bottom-4 right-4 flex h-16 w-16 touch-none select-none items-center justify-center rounded-full border-2 border-white/40 bg-black/25 text-2xl text-white active:bg-white/30"
        onContextMenu={(e) => e.preventDefault()}
        onPointerDown={(e) => {
          e.preventDefault();
          if (jumpFinger.current !== null) return;
          jumpFinger.current = e.pointerId;
          e.currentTarget.setPointerCapture(e.pointerId);
          key("Space", true);
        }}
        onPointerUp={jumpOff}
        onPointerCancel={jumpOff}
        onLostPointerCapture={jumpOff}
      >
        ⤒
      </button>
    </div>
  ) : (
    <div className="flex aspect-video w-full items-center justify-center bg-black px-6 text-center text-sm text-zinc-500">{emptyText}</div>
  );
}
