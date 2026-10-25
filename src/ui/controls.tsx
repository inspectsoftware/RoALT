import { useEffect, useState } from "react";
import { toast } from "./Toast";

export function Switch({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <button type="button" role="switch" className="switch" aria-checked={checked} aria-label={label} onClick={() => onChange(!checked)} />
  );
}

/** A row of mutually exclusive buttons (the design's "seg" control). */
export function Segmented<T extends string | number>({ label, value, options, onChange, mono }: {
  label: string;
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  mono?: boolean;
}) {
  return (
    <div role="group" aria-label={label} className="seg-group">
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          className={mono ? "seg mono" : "seg"}
          aria-pressed={o.value === value}
          onClick={() => o.value !== value && onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

/** Number input that saves on blur or Enter and snaps back when out of range or off-step. */
export function NumberField({ label, unit, short, min, max, step = 1, value, onCommit }: {
  /** Shown before the field; else `unit` names it for screen readers only. */
  label?: string;
  unit: string;
  short: string;
  min: number;
  max: number;
  step?: number;
  value: number;
  onCommit: (n: number) => void;
}) {
  const [text, setText] = useState(String(value));
  useEffect(() => setText(String(value)), [value]);
  const commit = () => {
    const n = Number(text);
    if (n === value) return;
    if (Number.isInteger(n / step) && n >= min && n <= max) onCommit(n);
    else {
      toast(step === 1 ? `Use a whole number of ${unit} from ${min} to ${max}.` : `Use ${min} to ${max} ${unit}, in steps of ${step}.`, "error");
      setText(String(value));
    }
  };
  return (
    <label className="flex items-center gap-2">
      {label ? <span>{label}</span> : <span className="sr-only">{unit}</span>}
      <input
        className="field mono w-20"
        type="number"
        min={min}
        max={max}
        step={step}
        value={text}
        onChange={(e) => setText(e.target.value)}
        onBlur={commit}
        onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
      />
      <span className="text-dim">{short}</span>
    </label>
  );
}
