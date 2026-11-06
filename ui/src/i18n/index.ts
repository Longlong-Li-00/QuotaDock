import { en, type Dict } from "./en.ts";
import { zhCN } from "./zh-CN.ts";
import type { Lang } from "../api/types.ts";

export type { Dict };

export function dictFor(lang: Lang): Dict {
  return lang === "zh-CN" ? zhCN : en;
}

/** Looks up a dynamic key (e.g. `reason_${code}`), falling back to the raw code. */
export function lookup(t: Dict, key: string, fallback?: string): string {
  const v = (t as Record<string, unknown>)[key];
  return typeof v === "string" ? v : (fallback ?? key);
}
