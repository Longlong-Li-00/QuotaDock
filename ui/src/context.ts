import { createContext, useContext } from "react";
import type { DashboardDto, Lang, SettingsDto } from "./api/types.ts";
import type { Dict } from "./i18n/en.ts";
import { en } from "./i18n/en.ts";

export interface AppCtx {
  t: Dict;
  lang: Lang;
  /** Re-rendered every second for "x sec ago" labels; no backend request is involved. */
  now: number;
  dashboard: DashboardDto | null;
  settings: SettingsDto | null;
  reload: () => void;
  saveSettings: (next: SettingsDto["settings"]) => Promise<void>;
}

export const AppContext = createContext<AppCtx>({
  t: en,
  lang: "en",
  now: Date.now(),
  dashboard: null,
  settings: null,
  reload: () => {},
  saveSettings: async () => {},
});

export const useApp = () => useContext(AppContext);
