export function RangeField({
  label,
  value,
  unit,
  displayValue,
  min,
  max,
  step,
  onChange,
  note,
}: {
  label: string;
  value: number;
  unit: string;
  displayValue?: string;
  min: number;
  max: number;
  step: number;
  onChange: (value: number) => void;
  note?: string;
}) {
  return (
    <label className="range-field">
      <span>
        {label}
        <output>{displayValue ?? `${value}${unit}`}</output>
      </span>
      <input
        aria-label={label}
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(event) => onChange(Number(event.target.value))}
      />
      {note && <small>{note}</small>}
    </label>
  );
}
