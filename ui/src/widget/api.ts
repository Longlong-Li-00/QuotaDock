// The widget page's whole API: four widget-only commands (src-tauri/capabilities/widget.json). No
// settings, refresh, provider or generic window access. In a plain browser (design preview) a
// mock with fixed scenarios is used instead.

import { invoke } from "@tauri-apps/api/core";
import type { WidgetView } from "../api/types.ts";
import { mockWidgetApi } from "./mock.ts";

export interface WidgetApi {
  /** The current view (local data only; never a provider request). */
  getView(): Promise<WidgetView>;
  openMain(): Promise<void>;
  /** Starts a system drag; false while the widget is locked (decided by the backend). */
  startDrag(): Promise<boolean>;
  /** The native right-click menu (open, lock, always on top, detailed view, hide). */
  contextMenu(): Promise<void>;
}

const tauriApi: WidgetApi = {
  getView: () => invoke("get_widget_view"),
  openMain: () => invoke("open_main_window"),
  startDrag: () => invoke("widget_start_drag"),
  contextMenu: () => invoke("widget_context_menu"),
};

const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export const widgetApi: WidgetApi = inTauri ? tauriApi : mockWidgetApi;
