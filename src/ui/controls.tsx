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
