// Browser previews only: what the Rust `display_tone` would send. Pages never compute a tone.

import type { DisplayTone } from "./types.ts";

export function mockTone(remaining: number | null, current: boolean): DisplayTone {
  if (remaining === null) return "unknown";
  if (remaining <= 0) return "empty";
  if (remaining <= 10) return "critical";
  if (remaining <= 30) return "low";
  if (remaining <= 50) return "caution";
  return current ? "normal" : "muted";
}
