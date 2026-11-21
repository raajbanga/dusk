import { render, type ComponentChildren } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Slider } from "./Slider";
import "./index.css";

type State = {
  night_light: number | null;
  brightness: number | null;
  accent: { light: string; dark: string } | null;
};

type Setting = "night_light" | "brightness";
const STEP = 5;

/** Sends the latest value at most once per frame while dragging; the final value always goes through on release. */
function useApply(setting: Setting) {
  const command = `set_${setting}`;
  const pending = useRef<number | null>(null);
  const frame = useRef(0);
  return {
    preview(value: number) {
      pending.current = value;
      if (frame.current) return;
      frame.current = requestAnimationFrame(() => {
        frame.current = 0;
        if (pending.current !== null) invoke(command, { value: pending.current, commit: false });
      });
    },
    commit(value: number) {
      cancelAnimationFrame(frame.current);
      frame.current = 0;
      pending.current = null;
      invoke(command, { value, commit: true });
    },
  };
}

function StepButton({ label, onClick, children }: { label: string; onClick: () => void; children: ComponentChildren }) {
  return (
    <button
      type="button"
      aria-label={label}
      onClick={onClick}
      class="grid size-6 place-items-center rounded-[4px] text-text outline-none hover:bg-hover active:bg-press active:opacity-80 focus-visible:ring-2 focus-visible:ring-text"
    >
      <svg viewBox="0 0 12 12" class="size-3" fill="none" stroke="currentColor" stroke-width="1.25" stroke-linecap="round">
        {children}
      </svg>
    </button>
  );
}

function Row({ label, setting, initial }: { label: string; setting: Setting; initial: number }) {
  const [value, setValue] = useState(initial);
  // After a - / + press, show the value briefly so it's clear where the slider landed.
  const [flash, setFlash] = useState(false);
  const flashTimer = useRef(0);
  const apply = useApply(setting);
  const step = (delta: number) => {
    const v = Math.min(100, Math.max(0, value + delta));
    setValue(v);
    apply.commit(v);
    setFlash(true);
    clearTimeout(flashTimer.current);
    flashTimer.current = setTimeout(() => setFlash(false), 1000);
  };

  return (
    <div>
      <div class="flex h-6 items-center justify-between">
        <span class="text-sm text-text">{label}</span>
        <div class="flex">
          <StepButton label={`Decrease ${label}`} onClick={() => step(-STEP)}>
            <path d="M2.5 6h7" />
          </StepButton>
          <StepButton label={`Increase ${label}`} onClick={() => step(STEP)}>
            <path d="M2.5 6h7M6 2.5v7" />
          </StepButton>
        </div>
      </div>
      <Slider
        label={label}
        value={value}
        showValue={flash}
        onPreview={(v) => {
          setValue(v);
          apply.preview(v);
        }}
        onCommit={(v) => {
          setValue(v);
          apply.commit(v);
        }}
      />
    </div>
  );
}

function App() {
  const [state, setState] = useState<State | null>(null);
  const panel = useRef<HTMLDivElement>(null);

  // The window is created per open (or on hover): load values, render, then report our measured size.
  useEffect(() => {
    invoke<State>("get_state").then(setState);
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && getCurrentWindow().destroy();
    addEventListener("keydown", onKey);
    return () => removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    if (!state || !panel.current) return;
    if (state.accent) {
      const dark = matchMedia("(prefers-color-scheme: dark)").matches;
      document.documentElement.style.setProperty("--accent", dark ? state.accent.light : state.accent.dark);
    }
    const { width, height } = panel.current.getBoundingClientRect();
    invoke("panel_ready", { width, height });
  }, [state]);

  return (
    <div ref={panel} class="inline-flex w-[320px] flex-col gap-2 bg-tint px-4 pt-2 pb-1.5">
      {state?.night_light != null && <Row label="Night Light" setting="night_light" initial={state.night_light} />}
      {state?.brightness != null && <Row label="Brightness" setting="brightness" initial={state.brightness} />}
    </div>
  );
}

render(<App />, document.getElementById("root")!);
