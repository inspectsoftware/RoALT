import { useState, type FormEvent } from "react";
import { open as openFile, save as saveFile } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { api, asError, type Profile, type Settings } from "../../api";
import { useSetSettings } from "../../data";
import { NumberField, Segmented, Switch } from "../../ui/controls";
import { Dialog } from "../../ui/Dialog";
import { fmtNum, t, type Key } from "../../i18n";
import { friendly, toast, toastError } from "../../ui/Toast";
import { BandwidthRow, Heading, Row } from "../settings/SettingsView";
import { PREVIEW_FPS, setPreviewFps, storedFps, usePreviewFps, type PreviewFps } from "../dashboard/previewFps";

const profileFilter = () => [{ name: t("performance.profileFilter"), extensions: ["json"] }];
/** A share code: this prefix, then the profile's JSON as base64url. */
const CODE = "RoALT-PERF:";

function toCode(json: string) {
  const bytes = new TextEncoder().encode(json);
  return CODE + btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/** The profile's JSON in a share code; null when it isn't one. */
function fromCode(code: string): string | null {
  const body = code.trim();
  if (!body.startsWith(CODE)) return null;
  try {
    const b64 = body.slice(CODE.length).replace(/\s+/g, "").replace(/-/g, "+").replace(/_/g, "/");
    return new TextDecoder("utf-8", { fatal: true }).decode(Uint8Array.from(atob(b64), (c) => c.charCodeAt(0)));
  } catch {
    return null;
  }
}

/** Everything that changes how much the clients and RoALT itself use, and profiles to share it. */
export function PerformanceView({ settings }: { settings: Settings }) {
  const set = useSetSettings();
  const [previewFps, setFps] = usePreviewFps();

  return (
    <main className="scroll flex-1 min-w-0 px-10 pt-6 pb-12">
      <div className="enter max-w-[760px] flex flex-col">
        <h1 className="text-xl font-semibold tracking-[-0.01em] mb-2">{t("performance.title")}</h1>
        <p className="text-dim max-w-[620px]">{t("performance.intro")}</p>

        <Heading id="perf-clients">{t("performance.section.clients")}</Heading>
        <Row title={t("settings.fpsCap")} hint={t("settings.fpsCapHint")}>
          <NumberField unit={t("settings.unit.fpsOff")} short="FPS" min={0} max={360} value={settings.fpsCap} onCommit={(fpsCap) => set.mutate({ fpsCap })} />
        </Row>
        <Row title={t("settings.lowGraphics")} hint={t("settings.lowGraphicsHint")}>
          <Switch label={t("settings.lowGraphics")} checked={settings.lowGraphics} onChange={(lowGraphics) => set.mutate({ lowGraphics })} />
        </Row>
        <Row title={t("settings.lighterRendering")} hint={t("settings.lighterRenderingHint")}>
          <Switch label={t("settings.lighterRendering")} checked={settings.lighterRendering} onChange={(lighterRendering) => set.mutate({ lighterRendering })} />
        </Row>
        <Row title={t("settings.minimize")} hint={t("settings.minimizeHint")}>
          <Switch label={t("settings.minimize")} checked={settings.minimizeOnLaunch} onChange={(minimizeOnLaunch) => set.mutate({ minimizeOnLaunch })} />
        </Row>

        <Row title={t("settings.parkMinimized")} hint={t("settings.parkMinimizedHint")}>
          <Switch label={t("settings.parkMinimized")} checked={settings.parkMinimized} onChange={(parkMinimized) => set.mutate({ parkMinimized })} />
        </Row>

        <Heading id="perf-cpu">{t("performance.section.cpuMemory")}</Heading>
        <Row title={t("settings.memRestart")} hint={t("settings.memRestartHint")}>
          <NumberField unit={t("settings.unit.mbOff")} short="MB" min={0} max={65536} value={settings.memRestartMb} onCommit={(memRestartMb) => set.mutate({ memRestartMb })} />
        </Row>
        <Row title={t("settings.memThrottle")} hint={t("settings.memThrottleHint")}>
          <div className="flex flex-wrap items-center gap-3">
            <NumberField
              unit={t("settings.unit.mbOff")}
              short="MB"
              min={0}
              max={65536}
              value={settings.memThrottle.mb}
              onCommit={(mb) => set.mutate({ memThrottle: { ...settings.memThrottle, mb } })}
            />
            <Segmented
              mono
              label={t("settings.memThrottleCpu")}
              value={settings.memThrottle.cpuCap}
              options={[25, 10, 5, 2].map((c) => ({ value: c, label: t("settings.percent", { n: c }) }))}
              onChange={(cpuCap) => set.mutate({ memThrottle: { ...settings.memThrottle, cpuCap } })}
            />
          </div>
        </Row>
        <Row title={t("settings.trim")} hint={t("settings.trimHint")}>
          <NumberField unit={t("settings.unit.minutesOff")} short={t("settings.unit.min")} min={0} max={1440} value={settings.trimEveryMin} onCommit={(trimEveryMin) => set.mutate({ trimEveryMin })} />
        </Row>
        <Row title={t("settings.spread")} hint={t("settings.spreadHint")}>
          <Switch label={t("settings.spread")} checked={settings.autoSpread} onChange={(autoSpread) => set.mutate({ autoSpread })} />
        </Row>
        <Row title={t("settings.adaptive")} hint={t("settings.adaptiveHint")}>
          <Switch label={t("settings.adaptive")} checked={settings.adaptive.enabled} onChange={(enabled) => set.mutate({ adaptive: { ...settings.adaptive, enabled } })} />
        </Row>
        {settings.adaptive.enabled && (
          <Row title={t("settings.inBackground")} hint={t("settings.adaptiveBgHint")}>
            <div className="flex flex-wrap items-center gap-3">
              <Segmented
                mono
                label={t("settings.adaptiveBgCpu")}
                value={settings.adaptive.background.cpuCap ?? 0}
                options={[0, 50, 25, 10, 5].map((c) => ({ value: c, label: c ? t("settings.percent", { n: c }) : t("common.off") }))}
                onChange={(c) => set.mutate({ adaptive: { ...settings.adaptive, background: { ...settings.adaptive.background, cpuCap: c || null } } })}
              />
              <label className="flex items-center gap-2">
                <span className="text-xs text-dim">{t("settings.efficiency")}</span>
                <Switch
                  label={t("settings.adaptiveBgEco")}
                  checked={settings.adaptive.background.eco}
                  onChange={(eco) => set.mutate({ adaptive: { ...settings.adaptive, background: { ...settings.adaptive.background, eco } } })}
                />
              </label>
              <Segmented
                mono
                label={t("settings.adaptiveBgTrim")}
                value={settings.adaptive.background.trimEveryMin ?? -1}
                options={[-1, 0, 5, 15, 30, 60].map((m) => ({ value: m, label: m < 0 ? t("common.default") : m ? t("settings.minutesShort", { n: m }) : t("common.off") }))}
                onChange={(m) => set.mutate({ adaptive: { ...settings.adaptive, background: { ...settings.adaptive.background, trimEveryMin: m < 0 ? null : m } } })}
              />
            </div>
          </Row>
        )}

        <Heading id="perf-network">{t("performance.section.network")}</Heading>
        <BandwidthRow />

        <Heading id="perf-roalt">{t("performance.section.roalt")}</Heading>
        <Row title={t("dashboard.previewFps")} hint={t("dashboard.previewFpsHint")}>
          <Segmented mono label={t("dashboard.previewFps")} value={previewFps} options={PREVIEW_FPS.map((f) => ({ value: f, label: t("dashboard.fps", { fps: f }) }))} onChange={setFps} />
        </Row>

        <Heading id="perf-profiles">{t("performance.section.profiles")}</Heading>
        <ProfilesRow settings={settings} />
      </div>
    </main>
  );
}

const noThrottle = { mb: 0, cpuCap: 10 };
/** One-click setups: a profile each, reviewed like any other. */
const PRESETS: readonly (readonly [Key, Profile])[] = [
  ["performance.preset.maxAlts", {
    fpsCap: 15, lowGraphics: true, lighterRendering: true, minimizeOnLaunch: true, memRestartMb: 0, memThrottle: noThrottle, trimEveryMin: 15, autoSpread: true,
    adaptive: { enabled: true, background: { cpuCap: 25, eco: true, trimEveryMin: null, affinity: null } }, previewFps: 15,
  }],
  ["performance.preset.balanced", {
    fpsCap: 30, lowGraphics: true, lighterRendering: true, minimizeOnLaunch: false, memRestartMb: 0, memThrottle: noThrottle, trimEveryMin: 30, autoSpread: true,
    adaptive: { enabled: true, background: { cpuCap: 50, eco: true, trimEveryMin: null, affinity: null } }, previewFps: 15,
  }],
  ["performance.preset.defaults", {
    fpsCap: 0, lowGraphics: false, lighterRendering: false, minimizeOnLaunch: false, memRestartMb: 0, memThrottle: noThrottle, trimEveryMin: 0, autoSpread: false,
    adaptive: { enabled: false, background: { cpuCap: 25, eco: true, trimEveryMin: null, affinity: null } }, previewFps: 15,
  }],
];

/** Export, import and share the settings above. */
function ProfilesRow({ settings }: { settings: Settings }) {
  const [incoming, setIncoming] = useState<Profile | null>(null);
  const [pasteOpen, setPasteOpen] = useState(false);

  async function exportFile() {
    try {
      const path = await saveFile({ defaultPath: `RoALT-performance-${new Date().toISOString().slice(0, 10)}.json`, filters: profileFilter() });
      if (!path) return;
      await api.performanceExport(path, storedFps());
      toast(t("performance.exported"));
    } catch (e) {
      toastError(e);
    }
  }

  async function importFile() {
    try {
      const path = await openFile({ multiple: false, directory: false, filters: profileFilter() });
      if (path) setIncoming(await api.performanceRead(path));
    } catch (e) {
      toastError(e);
    }
  }

  async function copyCode() {
    try {
      await writeText(toCode(await api.performanceProfile(storedFps())));
      toast(t("performance.codeCopied"));
    } catch (e) {
      toastError(e);
    }
  }

  return (
    <>
    <Row title={t("performance.presets")} hint={t("performance.presetsHint")}>
      <div className="flex flex-wrap gap-2">
        {PRESETS.map(([label, profile]) => (
          <button key={label} className="btn" onClick={() => setIncoming(profile)}>{t(label)}</button>
        ))}
      </div>
    </Row>
    <Row title={t("performance.profile")} hint={t("performance.profileHint")} last>
      <div className="flex flex-wrap gap-2">
        <button className="btn" onClick={exportFile}>{t("performance.export")}</button>
        <button className="btn" onClick={importFile}>{t("performance.import")}</button>
        <button className="btn" onClick={copyCode}>{t("performance.copyCode")}</button>
        <button className="btn" onClick={() => setPasteOpen(true)}>{t("performance.pasteCode")}</button>
      </div>
      <PasteDialog open={pasteOpen} onClose={() => setPasteOpen(false)} onProfile={(p) => { setPasteOpen(false); setIncoming(p); }} />
      <ReviewDialog profile={incoming} settings={settings} onClose={() => setIncoming(null)} />
    </Row>
    </>
  );
}

function PasteDialog({ open, onClose, onProfile }: { open: boolean; onClose: () => void; onProfile: (p: Profile) => void }) {
  const [code, setCode] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    const json = fromCode(code);
    if (json === null) return setError(t("performance.badCode"));
    setBusy(true);
    try {
      onProfile(await api.performanceParse(json));
      setCode("");
      setError(null);
    } catch (err) {
      setError(friendly(asError(err)));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={open} onClose={onClose} title={t("performance.pasteTitle")}>
      <form onSubmit={submit} className="flex flex-col gap-4 p-5">
        <label className="flex flex-col gap-1.5">
          <span className="label">{t("performance.pasteLabel")}</span>
          <textarea
            className="field mono text-xs"
            rows={4}
            value={code}
            onChange={(e) => setCode(e.target.value)}
            autoComplete="off"
            autoCorrect="off"
            autoCapitalize="off"
            spellCheck={false}
            placeholder={`${CODE}…`}
            aria-invalid={!!error}
          />
        </label>
        {error && <div role="alert" className="alert">{error}</div>}
        <div className="flex justify-end gap-2 pt-1">
          <button type="button" className="btn" onClick={onClose}>{t("common.cancel")}</button>
          <button type="submit" className="btn btn-primary" disabled={busy || !code.trim()}>{t("performance.review")}</button>
        </div>
      </form>
    </Dialog>
  );
}

type Field = Exclude<keyof Profile, "previewFps">;
const LABELS: Record<Field, Key> = {
  fpsCap: "settings.fpsCap",
  lowGraphics: "settings.lowGraphics",
  lighterRendering: "settings.lighterRendering",
  minimizeOnLaunch: "settings.minimize",
  parkMinimized: "settings.parkMinimized",
  memRestartMb: "settings.memRestart",
  memThrottle: "settings.memThrottle",
  trimEveryMin: "settings.trim",
  autoSpread: "settings.spread",
  adaptive: "settings.adaptive",
};

const pct = (n: number | null) => (n ? t("settings.percent", { n }) : t("common.off"));

/** A setting's value as the review shows it. */
function shown(k: Field, v: Profile[Field] | Settings[Field]): string {
  if (v === undefined || v === null) return "";
  if (typeof v === "boolean") return v ? t("common.on") : t("common.off");
  if (typeof v === "number") return k === "memRestartMb" && v ? `${fmtNum(v)} MB` : fmtNum(v);
  if ("mb" in v) return `${v.mb ? `${fmtNum(v.mb)} MB` : t("common.off")} · ${pct(v.cpuCap)}`;
  const b = v.background;
  const trim = b.trimEveryMin === null ? t("common.default") : b.trimEveryMin ? t("settings.minutesShort", { n: b.trimEveryMin }) : t("common.off");
  return `${v.enabled ? t("common.on") : t("common.off")} · ${pct(b.cpuCap)} · ${t("settings.efficiency")} ${b.eco ? t("common.on") : t("common.off")} · ${trim}`;
}

function ReviewDialog({ profile, settings, onClose }: { profile: Profile | null; settings: Settings; onClose: () => void }) {
  const set = useSetSettings();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const changes = profile
    ? (Object.keys(LABELS) as Field[])
        .filter((k) => profile[k] !== undefined && profile[k] !== null && JSON.stringify(profile[k]) !== JSON.stringify(settings[k]))
        .map((k) => ({ label: t(LABELS[k]), from: shown(k, settings[k]), to: shown(k, profile[k]) }))
    : [];
  const fps = profile?.previewFps as PreviewFps | undefined;
  if (fps !== undefined && fps !== null && fps !== storedFps()) {
    changes.push({ label: t("dashboard.previewFps"), from: t("dashboard.fps", { fps: storedFps() }), to: t("dashboard.fps", { fps }) });
  }

  async function apply() {
    if (!profile) return;
    setBusy(true);
    setError(null);
    try {
      const { previewFps, ...patch } = profile;
      await set.mutateAsync(patch);
      if (previewFps) setPreviewFps(previewFps as PreviewFps);
      toast(t("performance.applied"));
      onClose();
    } catch (e) {
      setError(friendly(asError(e)));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog open={profile !== null} onClose={onClose} title={t("performance.reviewTitle")} width={480}>
      <div className="flex flex-col gap-4 p-5">
        {changes.length === 0 ? (
          <p className="text-dim">{t("performance.profileSame")}</p>
        ) : (
          <>
            <p className="text-dim">{t("performance.reviewIntro")}</p>
            <ul className="flex flex-col border border-line">
              {changes.map((c) => (
                <li key={c.label} className="flex items-baseline justify-between gap-4 px-3 py-2 border-b border-line last:border-b-0">
                  <span>{c.label}</span>
                  <span className="mono text-xs text-right break-all">
                    <span className="text-dim">{c.from}</span> → {c.to}
                  </span>
                </li>
              ))}
            </ul>
          </>
        )}
        {error && <div role="alert" className="alert">{error}</div>}
        <div className="flex justify-end gap-2 pt-1">
          <button className="btn" onClick={onClose}>{changes.length ? t("common.cancel") : t("common.close")}</button>
          {changes.length > 0 && (
            <button className="btn btn-primary" onClick={apply} disabled={busy} data-autofocus>
              {t("performance.apply")}
            </button>
          )}
        </div>
      </div>
    </Dialog>
  );
}
