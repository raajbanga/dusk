import { render } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { invoke } from "@tauri-apps/api/core";
import "./index.css";

type State = {
  night_light: number | null;
  brightness: number | null;
  accent: { light: string; dark: string } | null;
};

type Setting = "night_light" | "brightness";
const SETTINGS: [Setting, string][] = [
  ["night_light", "Night Light"],
  ["brightness", "Brightness"],
];
const STEP = 5;

addEventListener("keydown", (e) => e.key === "Escape" && invoke("close_panel"));

function StepButton({ label, icon, onClick }: { label: string; icon: string; onClick: () => void }) {
  return (
    <button
      type="button"
      aria-label={label}
      onClick={onClick}
      class="grid size-6 place-items-center rounded-[4px] text-text outline-none hover:bg-hover active:bg-press active:opacity-80 focus-visible:ring-2 focus-visible:ring-text"
    >
      <svg viewBox="0 0 12 12" class="size-3" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round">
        <path d={icon} />
      </svg>
    </button>
  );
}

/**
 * One setting: label, - / + buttons and a Windows 11 (Fluent) slider on a native range input
 * (track, ringed thumb: see index.css) with a value tooltip above the thumb. Both rows are this
 * component; only `setting`, the value `set_value` targets, differs.
 */
function Row({ label, setting, initial }: { label: string; setting: Setting; initial: number }) {
  const [value, setValue] = useState(initial);
  const [dragging, setDragging] = useState(false);
  // After a - / + press, show the value briefly so it's clear where the slider landed.
  const [flash, setFlash] = useState(false);
  const flashTimer = useRef(0);
  // While dragging, send the latest value at most once per frame; the final value always goes through.
  const latest = useRef(0);
  const frame = useRef(0);

  const preview = (v: number) => {
    setValue((latest.current = v));
    frame.current ||= requestAnimationFrame(() => {
      frame.current = 0;
      invoke("set_value", { setting, value: latest.current, commit: false });
    });
  };
  const commit = (v: number) => {
    setValue(v);
    cancelAnimationFrame(frame.current);
    frame.current = 0;
    invoke("set_value", { setting, value: v, commit: true });
  };
  const step = (delta: number) => {
    commit(Math.min(100, Math.max(0, value + delta)));
    setFlash(true);
    clearTimeout(flashTimer.current);
    flashTimer.current = setTimeout(() => setFlash(false), 1000);
  };

  return (
    <div>
      <div class="flex h-6 items-center justify-between">
        <span class="text-sm text-text">{label}</span>
        <div class="flex">
          <StepButton label={`Decrease ${label}`} icon="M2.5 6h7" onClick={() => step(-STEP)} />
          <StepButton label={`Increase ${label}`} icon="M2.5 6h7M6 2.5v7" onClick={() => step(STEP)} />
        </div>
      </div>
      <div class="relative" style={{ "--v": `${value}%` }}>
        <input
          type="range"
          class="slider"
          aria-label={label}
          min={0}
          max={100}
          value={value}
          onPointerDown={() => {
            setDragging(true);
            addEventListener("pointerup", () => setDragging(false), { once: true });
          }}
          onInput={(e) => preview(e.currentTarget.valueAsNumber)}
          onChange={(e) => commit(e.currentTarget.valueAsNumber)}
        />
        {(dragging || flash) && (
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
    </div>
  );
}

function App() {
  const [state, setState] = useState<State | null>(null);
  const panel = useRef<HTMLDivElement>(null);

  // The window is created per open (or on hover): load values, render, then report our measured size.
  useEffect(() => void invoke<State>("get_state").then(setState), []);

  useEffect(() => {
    if (!state || !panel.current) return;
    if (state.accent) {
      const root = document.documentElement.style;
      root.setProperty("--accent-light", state.accent.light);
      root.setProperty("--accent-dark", state.accent.dark);
    }
    const { width, height } = panel.current.getBoundingClientRect();
    invoke("panel_ready", { width, height });
  }, [state]);

  return (
    <div ref={panel} class="inline-flex w-[320px] flex-col gap-2 bg-tint px-4 pt-2 pb-1.5">
      {SETTINGS.map(([setting, label]) => {
        const initial = state?.[setting];
        return initial != null && <Row key={setting} label={label} setting={setting} initial={initial} />;
      })}
    </div>
  );
}

render(<App />, document.getElementById("root")!);
