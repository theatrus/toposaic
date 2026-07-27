import type { GenerationSpec } from "../contracts";

type LabelFont = GenerationSpec["marker_settings"]["label_font"];

const LABEL_FONTS: Array<{ value: LabelFont; label: string }> = [
  { value: "atkinson_hyperlegible", label: "Atkinson Hyperlegible" },
  { value: "noto_sans", label: "Noto Sans" },
  { value: "b612_mono", label: "B612 Mono" },
];

export function LabelFontSelect({
  note,
  onChange,
  value,
}: {
  note?: string;
  onChange: (font: LabelFont) => void;
  value: LabelFont;
}) {
  return (
    <label className="font-select-field">
      <span>Label font</span>
      <span className="font-select-control">
        <select
          aria-label="Label font"
          onChange={(event) => onChange(event.target.value as LabelFont)}
          value={value}
        >
          {LABEL_FONTS.map((font) => (
            <option key={font.value} value={font.value}>
              {font.label}
            </option>
          ))}
        </select>
      </span>
      {note && <small>{note}</small>}
    </label>
  );
}
