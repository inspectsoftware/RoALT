import type { Entry } from "../settings/registry";

/** Macros' settings for the settings registry (search, jumps, rows), in screen order. Which macro
 * runs on launch is each account's own pick (`macroAssign`), not a setting. */
export const MACROS_ENTRIES: readonly Entry[] = [
  { id: "ahk", place: { view: "macros" }, label: "macros.ahk", field: "ahkPath", control: { kind: "custom" }, aliases: ["ahk", "autohotkey", "exe"] },
];
