import { useEffect, useRef, type CanvasHTMLAttributes } from "react";
import type { RemoteManager } from "../transport";

/** One client's live picture, drawn straight onto a canvas as frames arrive (no React render per frame). */
export function LivePreview({
  manager,
  pcId,
  instanceId,
  onFps,
  ...canvas
}: {
  manager: RemoteManager;
  pcId: string;
  instanceId: string;
  onFps?: (fps: number) => void;
} & CanvasHTMLAttributes<HTMLCanvasElement>) {
  const ref = useRef<HTMLCanvasElement>(null);
  const fpsRef = useRef(onFps);
  fpsRef.current = onFps;

  useEffect(() => {
    let frames = 0;
    let since = performance.now();
    return manager.watchPreview(pcId, instanceId, async (jpeg) => {
      const bitmap = await createImageBitmap(jpeg);
      const c = ref.current;
      if (c) {
        if (c.width !== bitmap.width || c.height !== bitmap.height) {
          c.width = bitmap.width;
          c.height = bitmap.height;
        }
        c.getContext("2d")?.drawImage(bitmap, 0, 0);
      }
      bitmap.close();
      frames++;
      const now = performance.now();
      if (now - since >= 1000) {
        fpsRef.current?.(Math.round((frames * 1000) / (now - since)));
        frames = 0;
        since = now;
      }
    });
  }, [manager, pcId, instanceId]);

  return <canvas ref={ref} width={640} height={360} role="img" {...canvas} />;
}
