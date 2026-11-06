// The only way the UI talks to QuotaDock: the narrow Tauri commands. In a plain browser
// (design preview, screenshots) a mock backend with fixed scenarios is used instead.

import { invoke } from "@tauri-apps/api/core";
import type { AppSettings, ClaudeIntegrationView, DashboardDto, DisableOutcome, EventView, HistoryData, HistoryRange, RefreshDecision, SettingsDto } from "./types.ts";
import { mockBackend } from "./mock.ts";

export interface Backend {
  getDashboard(): Promise<DashboardDto>;
  refreshUsage(): Promise<RefreshDecision>;
  getUsageHistory(range: HistoryRange): Promise<HistoryData | null>;
  /** Every enabled provider on one timeline; `beforeMs` continues a page. */
  getQuotaEvents(includeDiagnostic: boolean, beforeMs: number | null, limit: number): Promise<EventView[]>;
  pauseMonitoring(): Promise<void>;
  resumeMonitoring(): Promise<void>;
  getSettings(): Promise<SettingsDto>;
  updateSettings(settings: AppSettings): Promise<SettingsDto>;
  acknowledgeCloseHint(dontShowAgain: boolean): Promise<void>;
  openDataFolder(): Promise<void>;
  exportDiagnostics(): Promise<string>;
  quitApp(): Promise<void>;
  getClaudeIntegration(): Promise<ClaudeIntegrationView>;
  enableClaudeIntegration(plannedAtMs: number, planId: string): Promise<ClaudeIntegrationView>;
  disableClaudeIntegration(): Promise<DisableOutcome>;
}

const tauriBackend: Backend = {
  getDashboard: () => invoke("get_dashboard"),
  refreshUsage: () => invoke("refresh_usage"),
  getUsageHistory: (range) => invoke("get_usage_history", { range }),
  getQuotaEvents: (includeDiagnostic, beforeMs, limit) => invoke("get_quota_events", { includeDiagnostic, beforeMs, limit }),
  pauseMonitoring: () => invoke("pause_monitoring"),
  resumeMonitoring: () => invoke("resume_monitoring"),
  getSettings: () => invoke("get_settings"),
  updateSettings: (settings) => invoke("update_settings", { settings }),
  acknowledgeCloseHint: (dontShowAgain) => invoke("acknowledge_close_hint", { dontShowAgain }),
  openDataFolder: () => invoke("open_data_folder"),
  exportDiagnostics: () => invoke("export_diagnostics"),
  quitApp: () => invoke("quit_app"),
  getClaudeIntegration: () => invoke("get_claude_integration"),
  enableClaudeIntegration: (plannedAtMs, planId) => invoke("enable_claude_integration", { plannedAtMs, planId }),
  disableClaudeIntegration: () => invoke("disable_claude_integration"),
};

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export const backend: Backend = inTauri ? tauriBackend : mockBackend;
