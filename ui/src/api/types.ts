// TypeScript mirror of the Rust DTOs (serde camelCase). The UI only renders these values;
// it never recomputes availability, schedules, cycles, forecasts or severities.

export type WindowType = "five_hour" | "weekly";
export type ResetSchedule = "confirmed" | "sliding" | "unconfirmed" | "passed";
export type Severity = "normal" | "warning" | "critical" | "error";
/** Colour of a remaining value, decided in Rust. Never read from or by the notification rules. */
export type DisplayTone = "normal" | "caution" | "low" | "critical" | "empty" | "muted" | "unknown";
export type Freshness = "no_data" | "fresh" | "maybe_stale" | "stale";
export type Risk = "insufficient_data" | "low" | "watch" | "at_risk";
export type ForecastStatus = "ok" | "insufficient_data" | "not_projected" | "reset_schedule_unconfirmed";
export type HealthState =
  | "connecting"
  | "ok"
  | "paused"
  | "not_found"
  | "not_authenticated"
  | "unsupported_version"
  | "reconnecting"
  | "stale"
  | "waiting"
  | "not_configured"
  | "api_billing"
  | "source_error"
  | "needs_verification"
  | "disabled";
export type ProviderId = "codex" | "claude";
export type RefreshCapability = "active" | "passive" | "unavailable";
export type LanguagePref = "system" | "zh-CN" | "en";
export type Lang = "zh-CN" | "en";
export type ThemePref = "system" | "light" | "dark";
export type RefreshDecision = "started" | "coalesced" | "throttled" | "paused" | "stopped";
export type HistoryRange = "six_hours" | "twelve_hours" | "day" | "week" | "month" | "current_cycle";

export interface CycleView {
  cycleKey: string;
  windowType: string;
  status: string;
  observedStartedAtMs: number;
  observedEndedAtMs: number | null;
  lastSeenAtMs: number;
  initialUsedPercent: number | null;
  finalUsedPercent: number | null;
  nominalWindowStartMs: number | null;
  startReason: string;
  endReason: string | null;
  confidence: string;
}

export interface WindowStatus {
  windowType: WindowType;
  present: boolean;
  usedPercent: number | null;
  remainingPercent: number | null;
  reportedResetAtMs: number | null;
  confirmedResetAtMs: number | null;
  resetSchedule: ResetSchedule;
  observationState: string | null;
  candidateState: string | null;
  slidingAnchor: boolean;
  snapshotQuality: string;
  currentCycle: CycleView | null;
  /** Codex: null. Claude: 1000 (status line) or 60000 (`claude -p /usage`); above 1000 the time is approximate. */
  resetPrecisionMs: number | null;
}

export interface PollSummary {
  snapshots: number;
  pollRuns: number;
  failedPolls: number;
  lastStatus: string | null;
  lastError: string | null;
  lastPollAtMs: number | null;
}

export interface CurrentUsage {
  provider: string;
  capturedAtMs: number | null;
  availability: string | null;
  ordinaryUsageAllowed: boolean | null;
  windows: WindowStatus[];
  resetCreditCount: number | null;
  bucketIds: string[];
  account: string | null;
  polls: PollSummary;
  classifierVersion: string;
}

export interface BurnMetrics {
  status: "insufficient_data" | "stable" | "consuming";
  recent6hRate: number | null;
  recent24hRate: number | null;
  currentCycleRate: number | null;
  points: number;
  spanMs: number;
  observedConsumptionPp: number;
  latestUsedPercent: number | null;
  latestCapturedAtMs: number | null;
}

export interface UsageForecast {
  windowType: WindowType;
  status: ForecastStatus;
  burnRatePerHour: number | null;
  burnRatePerDay: number | null;
  rateSource: string | null;
  currentUsedPercent: number | null;
  remainingPercent: number | null;
  estimatedExhaustionAtMs: number | null;
  serverResetAtMs: number | null;
  resetSchedule: ResetSchedule;
  marginHours: number | null;
  requiredSpanMs: number;
  pressure: number | null;
  risk: Risk;
  confidence: "Low" | "Medium" | "High";
  burn: BurnMetrics;
  heuristic: string;
}

export interface TrayStatusModel {
  severity: Severity;
  icon: string;
  paused: boolean;
  availability: boolean | null;
  availabilityUnknownPersistent: boolean;
  fiveHourRemaining: number | null;
  weeklyRemaining: number | null;
  updatedAtMs: number | null;
  freshness: Freshness;
  minutesSinceUpdate: number | null;
  reasons: string[];
  tooltipLines: string[];
  providers: ProviderTrayLine[];
}

export interface ProviderTrayLine {
  provider: ProviderId;
  displayName: string;
  severity: Severity;
  reasons: string[];
  fiveHourRemaining: number | null;
  weeklyRemaining: number | null;
  fiveHourTone: DisplayTone;
  weeklyTone: DisplayTone;
  live: boolean;
  hasData: boolean;
  note: string | null;
}

export interface ExtraLimitStatus {
  key: string;
  label: string;
  usedPercent: number;
  remainingPercent: number;
  resetsAtMs: number | null;
  resetPrecisionMs: number | null;
  current: boolean;
}

export type ClaudeSourceState =
  | "disabled"
  | "paused"
  | "not_found"
  | "not_authenticated"
  | "api_billing"
  | "not_configured"
  | "waiting"
  | "live"
  | "stale"
  | "source_error"
  | "needs_verification";

export interface ProviderSourceInfo {
  state: ClaudeSourceState;
  lastSource: "statusline" | "active_cli" | null;
  lastUpdateMs: number | null;
  lastPassiveMs: number | null;
  lastActiveMs: number | null;
  live: boolean;
  integrationEnabled: boolean;
  activeEnabled: boolean;
  activeSuspended: boolean;
  activeNeedsVerification: boolean;
  activeNextAllowedMs: number | null;
  billing: "subscription" | "api_key" | "unknown";
  cliFound: boolean;
}

export interface PollingStatus {
  running: boolean;
  paused: boolean;
  foreground: boolean;
  consecutiveFailures: number;
  polls: number;
  lastSuccess: boolean | null;
  nextDelayMs: number | null;
}

export interface ProviderDashboard {
  provider: string;
  displayName: string;
  planType: string | null;
  cliVersion: string | null;
  appServerVersion: string | null;
  compatibilityMode: boolean;
  health: { state: HealthState; consecutiveFailures: number; lastError: string | null };
  usage: CurrentUsage;
  forecasts: UsageForecast[];
  extraLimits: ExtraLimitStatus[];
  refreshCapability: RefreshCapability;
  source: ProviderSourceInfo | null;
}

export interface NotificationSettings {
  weeklyQuota: boolean;
  fiveHourQuota: boolean;
  forecastWeekly: boolean;
  forecastFiveHour: boolean;
  availabilityRestored: boolean;
  quotaRefresh: boolean;
  resetCredit: boolean;
  allowanceRefresh: boolean;
  providerErrors: boolean;
  claude: ClaudeNotificationSettings;
}

export interface ClaudeNotificationSettings {
  weeklyQuota: boolean;
  fiveHourQuota: boolean;
  forecastWeekly: boolean;
  allowanceRefresh: boolean;
}

export interface CodexSettings {
  enabled: boolean;
  showFiveHour: boolean;
}

export interface ClaudeSettings {
  enabled: boolean;
  showFiveHour: boolean;
  /** Absent = never chosen by the user = on (the backend applies the same rule). */
  activeRefresh?: boolean | null;
}

export type WidgetMode = "compact" | "detailed";

/** Where the widget was left: a logical offset inside the named monitor's work area. */
export interface WidgetPosition {
  monitor: string;
  x: number;
  y: number;
}

export interface DesktopWidgetSettings {
  enabled: boolean;
  alwaysOnTop: boolean;
  /** Background opacity in percent, 50-100. */
  opacity: number;
  mode: WidgetMode;
  rememberPosition: boolean;
  locked: boolean;
  /** Saved by the backend after a drag; a page's copy is ignored when settings are saved. */
  position?: WidgetPosition | null;
}

export interface AppSettings {
  language: LanguagePref;
  theme: ThemePref;
  closeToTray: boolean;
  closeHintDismissed: boolean;
  launchAtStartup: boolean;
  showDiagnostics: boolean;
  notifications: NotificationSettings;
  codex: CodexSettings;
  claude: ClaudeSettings;
  desktopWidget: DesktopWidgetSettings;
}

export interface SettingsDto {
  settings: AppSettings;
  effectiveLanguage: Lang;
  dataFolder: string;
}

export interface DashboardDto {
  generatedAtMs: number;
  paused: boolean;
  polling: PollingStatus | null;
  tray: TrayStatusModel;
  providers: ProviderDashboard[];
  settings: AppSettings;
  effectiveLanguage: Lang;
}

export interface EventView {
  id: number;
  provider: ProviderId;
  eventKey: string;
  windowType: string | null;
  eventType: string;
  visibility: "user" | "diagnostic";
  observedAtMs: number;
  confidence: "low" | "medium" | "high";
  confirmed: boolean;
  evidence: Record<string, unknown>;
  supersededByEventKey: string | null;
}

export interface HistoryPoint {
  t: number;
  remaining: number;
  used: number;
  segment: number;
  gapBefore: boolean;
}

export interface HistoryData {
  range: HistoryRange;
  fromMs: number;
  toMs: number;
  resolutionMs: number | null;
  series: { provider: ProviderId; windowType: WindowType; points: HistoryPoint[] }[];
  markers: { provider: ProviderId; t: number; windowType: string | null; eventType: string; confidence: string }[];
  sourceSnapshots: number;
}

export interface IntegrationPlan {
  settingsPath: string;
  settingsExists: boolean;
  currentStatusLine: Record<string, unknown> | null;
  newStatusLine: Record<string, unknown>;
  chainsExisting: boolean;
  bridgeInstallPath: string;
  backupPath: string;
  plannedAtMs: number;
  planId: string;
}

export interface IntegrationState {
  enabled: boolean;
  installedAtMs: number;
  settingsPath: string;
  previousStatusLine: Record<string, unknown> | null;
  installedStatusLine: Record<string, unknown>;
  backupPath: string | null;
}

export interface ClaudeIntegrationView {
  enabled: IntegrationState | null;
  plan: IntegrationPlan | null;
  planError: string | null;
}

export type DisableOutcome = "restored" | "changed_by_user" | "nothing_to_restore";

// ---------------------------------------------------------------- desktop widget (Phase 3.7)

export type WidgetProviderState = "live" | "stale" | "waiting" | "paused" | "error";
export type WidgetAvailability = "allowed" | "blocked" | "unknown";
/** Why the values are not current, for the words beside them. Display only. */
export type WidgetStaleKind = "maybe" | "stale" | "not_live";

/** A window's reset, from the same `WindowStatus` the dashboard shows. */
export interface WidgetReset {
  schedule: ResetSchedule | "missing";
  /** Only with "confirmed": nothing else may become a countdown. */
  atMs: number | null;
  precisionMs: number | null;
}

export interface WidgetProvider {
  id: ProviderId;
  displayName: string;
  shortLabel: string;
  fiveHourRemaining: number | null;
  weeklyRemaining: number | null;
  fiveHourTone: DisplayTone;
  weeklyTone: DisplayTone;
  fiveHourReset: WidgetReset;
  weeklyReset: WidgetReset;
  /** False: no 5-hour cell or bar for this provider (a null value with true shows a dash). */
  showFiveHour: boolean;
  state: WidgetProviderState;
  /** Null: the values are current, or there are none. */
  staleKind: WidgetStaleKind | null;
  /** Codex: from ordinaryUsageAllowed only. Claude: always "unknown" (never inferred). */
  availability: WidgetAvailability;
  severity: Severity;
  updatedAtMs: number | null;
  /** Already in the app's language. */
  note: string | null;
  /** `note` in a form that fits the compact name slot, when the full words do not. Null otherwise, a limit included. */
  noteShort: string | null;
  /** Only on a waiting row: why it has no data when that is not really a wait. Null: the true wait. */
  waitingNote: string | null;
}

/** Everything the widget shows, built by the backend from the tray model (no raw snapshots or settings). */
export interface WidgetView {
  generatedAtMs: number;
  paused: boolean;
  globalSeverity: Severity;
  language: Lang;
  theme: ThemePref;
  mode: WidgetMode;
  /** 50-100. */
  opacity: number;
  locked: boolean;
  providers: WidgetProvider[];
  updatedAtMs: number | null;
  /** Provider id -> data URL of the user's own ring icon (data folder, icons/<id>.svg|png). */
  customIcons: Record<string, string>;
}
