import type { ReactNode } from "react";

/** Shared form primitives for the wizard and the profile editor. */

const INPUT =
  "w-full rounded-md border border-line bg-base px-2.5 py-1.5 text-[13px] text-ink outline-none transition-colors placeholder:text-dim/70 focus:border-accent/60";

export function TextField({
  label,
  hint,
  className,
  ...props
}: {
  label: string;
  hint?: ReactNode;
} & React.InputHTMLAttributes<HTMLInputElement>) {
  return (
    <label className={`block ${className ?? ""}`}>
      <span className="mb-1 block text-[12px] font-medium text-dim">{label}</span>
      <input {...props} className={INPUT} />
      {hint && <span className="mt-1 block text-[11.5px] leading-relaxed text-dim">{hint}</span>}
    </label>
  );
}

/** Numeric input that never emits NaN (empty -> 0). */
export function NumberField({
  label,
  value,
  onValue,
  min = 1,
  max = 65535,
  hint,
  disabled,
  className,
}: {
  label: string;
  value: number;
  onValue: (n: number) => void;
  min?: number;
  max?: number;
  hint?: ReactNode;
  disabled?: boolean;
  className?: string;
}) {
  return (
    <label className={`block ${className ?? ""}`}>
      <span className="mb-1 block text-[12px] font-medium text-dim">{label}</span>
      <input
        type="number"
        value={value}
        min={min}
        max={max}
        disabled={disabled}
        onChange={(e) => onValue(Math.min(max, Math.max(0, Number(e.target.value) || 0)))}
        className={`${INPUT} font-mono disabled:opacity-50`}
      />
      {hint && <span className="mt-1 block text-[11.5px] text-dim">{hint}</span>}
    </label>
  );
}

/** Accessible switch row. */
export function Toggle({
  checked,
  onChange,
  label,
  description,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  label: string;
  description?: string;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      onClick={() => onChange(!checked)}
      className="flex w-full items-center justify-between gap-4 rounded-card px-1 py-1.5 text-left transition-colors hover:bg-base/50"
    >
      <span className="min-w-0">
        <span className="block text-[13px] font-medium">{label}</span>
        {description && (
          <span className="block text-[11.5px] leading-relaxed text-dim">{description}</span>
        )}
      </span>
      <span
        aria-hidden
        className={`relative h-5 w-9 shrink-0 rounded-full transition-colors ${
          checked ? "bg-accent" : "bg-line"
        }`}
      >
        <span
          className={`absolute top-0.5 h-4 w-4 rounded-full bg-base transition-all ${
            checked ? "left-[18px]" : "left-0.5"
          }`}
        />
      </span>
    </button>
  );
}

export function FieldRow({ children }: { children: ReactNode }) {
  return <div className="grid grid-cols-3 gap-3">{children}</div>;
}
