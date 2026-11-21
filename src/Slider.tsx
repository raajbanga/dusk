import { useState } from "preact/hooks";

type Props = {
  label: string;
  value: number;
  /** Show the value tooltip (it is always shown while dragging). */
  showValue: boolean;
  /** Fires continuously while dragging. */
  onPreview: (value: number) => void;
  /** Fires once the value has settled (pointer released, or a key press). */
  onCommit: (value: number) => void;
};

/**
 * Windows 11 (Fluent) slider on a native range input: 4px track with an accent fill,
 * a ringed thumb whose dot grows on hover and shrinks while pressed (see index.css),
 * and a value tooltip above the thumb.
 */
export function Slider({ label, value, showValue, onPreview, onCommit }: Props) {
  const [dragging, setDragging] = useState(false);
  const pct = `${value}%`;
  return (
    <div class="relative" style={{ "--v": pct }}>
      <input
        type="range"
        class="slider"
        aria-label={label}
        min={0}
        max={100}
        step={1}
        value={value}
        onPointerDown={() => {
          setDragging(true);
          addEventListener("pointerup", () => setDragging(false), { once: true });
        }}
        onInput={(e) => onPreview(e.currentTarget.valueAsNumber)}
        onChange={(e) => onCommit(e.currentTarget.valueAsNumber)}
      />
      {(dragging || showValue) && (
        <span
          role="status"
          // Thumb centre: the 20px thumb travels the track width minus its own size.
          style={{ left: `calc(10px + (100% - 20px) * ${value / 100})` }}
          class="pointer-events-none absolute bottom-[calc(50%+18px)] min-w-8 -translate-x-1/2 rounded-[4px] border border-tip-stroke bg-tip px-2 py-[3px] text-center text-xs text-text tabular-nums shadow-[0_4px_8px_rgb(0_0_0/0.14)]"
        >
          {value}
        </span>
      )}
    </div>
  );
}
