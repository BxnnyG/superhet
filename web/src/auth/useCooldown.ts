import { useCallback, useEffect, useState } from "react";

/** Counts down whole seconds once started; `remaining` is 0 when idle. */
export function useCooldown(): { remaining: number; start: (seconds: number) => void } {
  const [remaining, setRemaining] = useState(0);
  const active = remaining > 0;
  useEffect(() => {
    if (!active) return;
    const id = setInterval(() => setRemaining((r) => Math.max(0, r - 1)), 1000);
    return () => clearInterval(id);
  }, [active]);
  const start = useCallback((seconds: number) => setRemaining(Math.max(0, Math.ceil(seconds))), []);
  return { remaining, start };
}
