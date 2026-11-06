// Browser-only mock used for design previews and screenshots (never used inside Tauri).
// Scenario via ?scenario=normal|sliding|low|critical|blocked|unconfirmed|passed|passedread|missing|maybestale|stale|unknown|error|notfound|unauth|connecting|paused
// and ?claude=live|stale|waiting|notconfigured|apibilling|notfound|notauth|low|critical|approx|unconfirmed|passed|missing|off (Claude card).

import type { Backend } from "./backend.ts";
import { mockTone } from "./mockTone.ts";
import type {
  AppSettings,
  ClaudeIntegrationView,
  ClaudeSourceState,
  DashboardDto,
  ProviderDashboard,
  ProviderTrayLine,
  EventView,
  HistoryData,
  HistoryPoint,
  HistoryRange,
  Lang,
  SettingsDto,
  UsageForecast,
  WindowStatus,
} from "./types.ts";

const MIN = 60_000;
const HOUR = 60 * MIN;
const params = typeof location !== "undefined" ? new URLSearchParams(location.search) : new URLSearchParams();
const scenario = params.get("scenario") ?? "normal";
const claudeScenario = params.get("claude") ?? "live";
let integrationOn = !["notconfigured", "apibilling", "notfound", "notauth", "off"].includes(claudeScenario);

let settings: AppSettings = {
  language: (params.get("lang") as AppSettings["language"]) ?? "system",
  theme: (params.get("theme") as AppSettings["theme"]) ?? "system",
  closeToTray: true,
  closeHintDismissed: false,
  launchAtStartup: false,
  showDiagnostics: params.get("diag") === "1",
  notifications: {
    weeklyQuota: true,
    fiveHourQuota: false,
    forecastWeekly: true,
    forecastFiveHour: false,
    availabilityRestored: true,
    quotaRefresh: true,
    resetCredit: true,
    allowanceRefresh: false,
    providerErrors: true,
    claude: { weeklyQuota: true, fiveHourQuota: false, forecastWeekly: false, allowanceRefresh: false },
  },
  codex: { enabled: params.get("codex") !== "off", showFiveHour: !["codex", "both"].includes(params.get("five") ?? "") },
  claude: { enabled: claudeScenario !== "off", showFiveHour: !["claude", "both"].includes(params.get("five") ?? "") },
  desktopWidget: { enabled: params.get("widget") === "1", alwaysOnTop: true, opacity: 85, mode: "compact", rememberPosition: true, locked: false },
};
let paused = scenario === "paused";

function effective(): Lang {
  if (settings.language === "system") return navigator.language.startsWith("zh") ? "zh-CN" : "en";
  return settings.language;
}

function win(type: WindowStatus["windowType"], used: number | null, schedule: WindowStatus["resetSchedule"], resetIn: number | null, state: string): WindowStatus {
  const now = Date.now();
  return {
    windowType: type,
    present: used !== null,
    usedPercent: used,
    remainingPercent: used === null ? null : 100 - used,
    reportedResetAtMs: resetIn === null ? null : now + resetIn,
    confirmedResetAtMs: schedule === "confirmed" && resetIn !== null ? now + resetIn : null,
    resetSchedule: schedule,
    observationState: state,
    candidateState: "none",
    slidingAnchor: schedule === "sliding",
    snapshotQuality: used === null ? "missing_window" : "normal",
    currentCycle: {
      cycleKey: `mock|${type}`,
      windowType: type,
      status: "open",
      observedStartedAtMs: now - (type === "weekly" ? 26 * HOUR : 2 * HOUR),
      observedEndedAtMs: null,
      lastSeenAtMs: now,
      initialUsedPercent: used,
      finalUsedPercent: used,
      nominalWindowStartMs: null,
      startReason: type === "weekly" ? "first_observation" : "natural_rollover_observed",
      endReason: null,
      confidence: type === "weekly" ? "low" : "medium",
    },
    resetPrecisionMs: null,
  };
}

function forecast(type: WindowStatus["windowType"], over: Partial<UsageForecast>): UsageForecast {
  return {
    windowType: type,
    status: "insufficient_data",
    burnRatePerHour: null,
    burnRatePerDay: null,
    rateSource: null,
    currentUsedPercent: null,
    remainingPercent: null,
    estimatedExhaustionAtMs: null,
    serverResetAtMs: null,
    resetSchedule: "unconfirmed",
    marginHours: null,
    pressure: null,
    risk: "insufficient_data",
    confidence: "Low",
    burn: { status: "insufficient_data", recent6hRate: null, recent24hRate: null, currentCycleRate: null, points: 2, spanMs: 10 * MIN, observedConsumptionPp: 0, latestUsedPercent: null, latestCapturedAtMs: null },
    requiredSpanMs: 30 * MIN,
    heuristic: "QuotaDock heuristic, not an OpenAI metric",
    ...over,
  };
}

function dashboard(): DashboardDto {
  const now = Date.now();
  let windows: WindowStatus[] = [win("five_hour", 12, "confirmed", 2 * HOUR + 14 * MIN, "active"), win("weekly", 18, "confirmed", 6 * 24 * HOUR + 9 * HOUR, "active")];
  let allowed: boolean | null = true;
  let health: DashboardDto["providers"][number]["health"]["state"] = paused ? "paused" : "ok";
  let severity: DashboardDto["tray"]["severity"] = "normal";
  let reasons: string[] = [];
  const weeklyFc = forecast("weekly", {
    status: "ok",
    burnRatePerHour: 0.133,
    burnRatePerDay: 3.2,
    rateSource: "recent_24h",
    currentUsedPercent: 18,
    remainingPercent: 82,
    estimatedExhaustionAtMs: now + 25 * 24 * HOUR,
    serverResetAtMs: now + 6 * 24 * HOUR + 9 * HOUR,
    resetSchedule: "confirmed",
    marginHours: 440,
    pressure: 0.25,
    risk: "low",
    confidence: "Medium",
    burn: { status: "consuming", recent6hRate: 0.21, recent24hRate: 0.133, currentCycleRate: 0.12, points: 300, spanMs: 26 * HOUR, observedConsumptionPp: 16, latestUsedPercent: 18, latestCapturedAtMs: now },
  });
  let forecasts = [forecast("five_hour", {}), weeklyFc];
  if (scenario === "sliding") {
    windows = [win("five_hour", 0, "sliding", 5 * HOUR, "idle_or_uninitialized"), win("weekly", 2, "confirmed", 6 * 24 * HOUR + 18 * HOUR, "active")];
    forecasts = [forecast("five_hour", { resetSchedule: "sliding" }), forecast("weekly", { status: "not_projected", burnRatePerHour: 0, burnRatePerDay: 0, risk: "low", pressure: 0, burn: { ...weeklyFc.burn, status: "stable", recent24hRate: 0, currentCycleRate: 0, observedConsumptionPp: 0 } })];
  } else if (scenario === "low") {
    // Both windows low; the 5-hour reminder is off by default, so only weekly raises the level.
    windows = [win("five_hour", 72, "confirmed", 3 * HOUR, "active"), win("weekly", 78, "confirmed", 2 * 24 * HOUR + 5 * HOUR, "active")];
    forecasts = [forecast("five_hour", {}), { ...weeklyFc, currentUsedPercent: 78, remainingPercent: 22, burnRatePerDay: 15.1, burnRatePerHour: 0.629, estimatedExhaustionAtMs: now + 35 * HOUR, serverResetAtMs: now + 53 * HOUR, marginHours: -18, pressure: 1.49, risk: "at_risk" }];
    severity = "warning";
    reasons = ["weekly_remaining_30", "weekly_forecast_at_risk"];
  } else if (scenario === "critical") {
    // 4 % of 5-hour with its reminder off: critical tone, no reason of its own.
    windows = [win("five_hour", 96, "confirmed", 47 * MIN, "active"), win("weekly", 91, "confirmed", 2 * 24 * HOUR + 5 * HOUR, "active")];
    forecasts = [forecast("five_hour", {}), { ...weeklyFc, currentUsedPercent: 91, remainingPercent: 9, burnRatePerDay: 6.1, burnRatePerHour: 0.254, estimatedExhaustionAtMs: now + 35 * HOUR, serverResetAtMs: now + 53 * HOUR, marginHours: -18, pressure: 1.49, risk: "at_risk" }];
    severity = "critical";
    reasons = ["weekly_remaining_10", "weekly_forecast_at_risk"];
  } else if (scenario === "blocked") {
    windows = [win("five_hour", 100, "confirmed", 47 * MIN, "active"), win("weekly", 60, "confirmed", 4 * 24 * HOUR, "active")];
    allowed = false;
    severity = "critical";
    reasons = ["limit_reached"];
  } else if (scenario === "unconfirmed") {
    windows = [win("five_hour", 12, "unconfirmed", 2 * HOUR, "active"), win("weekly", 18, "unconfirmed", 6 * 24 * HOUR, "active")];
    forecasts = [forecast("five_hour", {}), forecast("weekly", { status: "reset_schedule_unconfirmed", risk: "insufficient_data" })];
  } else if (scenario === "passed") {
    // As in `clear_last_period_values`: the window and its time stay, the value from the last period does not.
    windows = [{ ...win("five_hour", 87, "passed", -4 * MIN, "expired_unconfirmed"), usedPercent: null, remainingPercent: null }, win("weekly", 18, "confirmed", 6 * 24 * HOUR + 9 * HOUR, "active")];
    forecasts = [forecast("five_hour", { resetSchedule: "passed" }), weeklyFc];
  } else if (scenario === "passedread") {
    // A read after that time: the service's current answer keeps its number.
    windows = [win("five_hour", 87, "passed", -4 * MIN, "expired_unconfirmed"), win("weekly", 18, "confirmed", 6 * 24 * HOUR + 9 * HOUR, "active")];
    forecasts = [forecast("five_hour", { resetSchedule: "passed" }), weeklyFc];
  } else if (scenario === "missing") {
    windows = [win("five_hour", null, "unconfirmed", null, "temporarily_missing"), win("weekly", 18, "confirmed", 6 * 24 * HOUR + 9 * HOUR, "active")];
  } else if (scenario === "maybestale") {
    windows = [win("five_hour", 18, "confirmed", 2 * HOUR, "active"), win("weekly", 78, "confirmed", 4 * 24 * HOUR, "active")];
    severity = "warning";
    reasons = ["weekly_remaining_30"];
  } else if (scenario === "stale") {
    // No valid read for 30 min: a high value goes muted, a low one stays low, the rule says error.
    windows = [win("five_hour", 18, "confirmed", 2 * HOUR, "active"), win("weekly", 78, "confirmed", 4 * 24 * HOUR, "active")];
    forecasts = [forecast("five_hour", {}), { ...weeklyFc, currentUsedPercent: 78, remainingPercent: 22, burnRatePerDay: 15.1, burnRatePerHour: 0.629, estimatedExhaustionAtMs: now + 35 * HOUR, serverResetAtMs: now + 4 * 24 * HOUR, marginHours: -61, pressure: 2.75, risk: "at_risk" }];
    severity = "error";
    reasons = ["no_valid_snapshot_30m", "weekly_remaining_30", "weekly_forecast_at_risk"];
  } else if (scenario === "unknown") {
    allowed = null;
    severity = "warning";
    reasons = ["availability_unknown_persistent"];
    windows = [win("five_hour", 12, "unconfirmed", 2 * HOUR, "active"), win("weekly", null, "unconfirmed", null, "temporarily_missing")];
  } else if (scenario === "error" || scenario === "notfound" || scenario === "unauth") {
    health = scenario === "error" ? "reconnecting" : scenario === "notfound" ? "not_found" : "not_authenticated";
    severity = "error";
    reasons = [scenario === "unauth" ? "not_authenticated" : "provider_unavailable"];
    if (scenario !== "error") {
      windows = [win("five_hour", null, "unconfirmed", null, "unknown"), win("weekly", null, "unconfirmed", null, "unknown")];
      allowed = null;
    }
  } else if (scenario === "connecting") {
    health = "connecting";
    windows = [win("five_hour", null, "unconfirmed", null, "unknown"), win("weekly", null, "unconfirmed", null, "unknown")];
    allowed = null;
  }
  const noData = scenario === "connecting" || scenario === "notfound" || scenario === "unauth";
  const claude = settings.claude.enabled ? claudeMock(now) : null;
  if (claude?.line.severity === "critical" && severity !== "error") severity = "critical";
  if (claude?.line.severity === "warning" && severity === "normal") severity = "warning";
  const ageMs = noData ? null : scenario === "error" ? 34 * MIN : scenario === "stale" ? 47 * MIN : scenario === "maybestale" ? 14 * MIN : paused ? 12 * MIN : 20_000;
  const freshness: DashboardDto["tray"]["freshness"] = ageMs === null ? "no_data" : ageMs > 30 * MIN ? "stale" : ageMs > 10 * MIN ? "maybe_stale" : "fresh";
  // As in `derive_tray_status`: rule-valid values only, and a passed reset has no current value.
  const toneCurrent = !paused && (freshness === "fresh" || freshness === "maybe_stale");
  const codexTone = (w: WindowStatus | undefined) =>
    mockTone(w && w.present && w.snapshotQuality === "normal" && (w.reportedResetAtMs === null || w.reportedResetAtMs > now) ? w.remainingPercent : null, toneCurrent);
  return {
    generatedAtMs: now,
    paused,
    polling: { running: true, paused, foreground: true, consecutiveFailures: scenario === "error" ? 3 : 0, polls: 42, lastSuccess: scenario !== "error", nextDelayMs: 60_000 },
    tray: {
      severity,
      icon: paused ? "paused" : severity,
      paused,
      availability: allowed,
      availabilityUnknownPersistent: scenario === "unknown",
      fiveHourRemaining: windows[0]?.remainingPercent ?? null,
      weeklyRemaining: windows[1]?.remainingPercent ?? null,
      updatedAtMs: ageMs === null ? null : now - ageMs,
      freshness,
      minutesSinceUpdate: ageMs === null ? null : Math.floor(ageMs / MIN),
      reasons,
      tooltipLines: [],
      providers: [
        { provider: "codex", displayName: "Codex", severity, reasons, fiveHourRemaining: windows[0]?.remainingPercent ?? null, weeklyRemaining: windows[1]?.remainingPercent ?? null, fiveHourTone: codexTone(windows[0]), weeklyTone: codexTone(windows[1]), live: freshness === "fresh", hasData: !noData, note: null },
        ...(claude ? [claude.line] : []),
      ],
    },
    providers: [
      {
        provider: "codex",
        displayName: "Codex",
        planType: noData ? null : "plus",
        cliVersion: "0.162.0-alpha.2",
        appServerVersion: noData ? null : "0.162.0-alpha.2",
        compatibilityMode: false,
        health: { state: health, consecutiveFailures: scenario === "error" ? 3 : 0, lastError: scenario === "error" ? "network_error: account/rateLimits/read timed out" : null },
        usage: {
          provider: "codex",
          capturedAtMs: ageMs === null ? null : now - ageMs,
          availability: allowed === null ? (noData ? null : "unknown") : allowed ? "allowed" : "blocked",
          ordinaryUsageAllowed: noData ? null : allowed,
          windows,
          resetCreditCount: noData ? null : 2,
          bucketIds: ["codex"],
          account: "a1b2c3d4",
          polls: { snapshots: 412, pollRuns: 415, failedPolls: 3, lastStatus: "success", lastError: null, lastPollAtMs: now - 20_000 },
          classifierVersion: "reset-detector-v3",
        },
        forecasts,
        extraLimits: [],
        refreshCapability: paused ? "unavailable" : "active",
        source: null,
      },
      ...(claude ? [claude.card] : []),
    ],
    settings,
    effectiveLanguage: effective(),
  };
}

function claudeMock(now: number): { card: ProviderDashboard; line: ProviderTrayLine } {
  const state: ClaudeSourceState =
    paused ? "paused"
    : (({ stale: "stale", waiting: "waiting", notconfigured: "not_configured", apibilling: "api_billing", notfound: "not_found", notauth: "not_authenticated" }) as Record<string, ClaudeSourceState>)[claudeScenario] ?? "live";
  const hasData = state === "live" || state === "stale" || state === "paused";
  const lastUpdate = state === "stale" ? now - 47 * MIN : now - 2 * MIN;
  const used5 = claudeScenario === "low" ? 74 : claudeScenario === "critical" ? 96 : 39;
  // A stale low value stays low (a value can only fall inside one window); stale 5-hour stays high: muted.
  const usedW = claudeScenario === "low" ? 76 : claudeScenario === "critical" ? 92 : claudeScenario === "stale" ? 78 : 28;
  const atRisk = claudeScenario === "low" || claudeScenario === "critical";
  // Status line: to the second. `claude -p /usage` (approx): to the minute, so "Expected to reset".
  const precision = claudeScenario === "approx" ? MIN : 1000;
  const cw = (type: WindowStatus["windowType"], used: number | null, schedule: WindowStatus["resetSchedule"], resetIn: number | null): WindowStatus => {
    const at = resetIn === null ? null : claudeScenario === "approx" ? Math.floor((now + resetIn) / MIN) * MIN : now + resetIn;
    return {
      ...win(type, used, schedule, resetIn, "active"),
      // As in `window_status`: a passed reset keeps its time, not its value.
      ...(schedule === "passed" ? { usedPercent: null, remainingPercent: null } : {}),
      reportedResetAtMs: at,
      confirmedResetAtMs: schedule === "confirmed" ? at : null,
      observationState: null,
      candidateState: null,
      currentCycle: null,
      snapshotQuality: used === null ? "none" : "normal",
      resetPrecisionMs: used === null ? null : precision,
    };
  };
  const five5 = claudeScenario === "missing" ? cw("five_hour", null, "unconfirmed", null) : claudeScenario === "passed" ? cw("five_hour", 87, "passed", -6 * MIN) : claudeScenario === "unconfirmed" ? cw("five_hour", used5, "unconfirmed", null) : cw("five_hour", used5, "confirmed", 3 * HOUR + 20 * MIN);
  const weeklyW = claudeScenario === "unconfirmed" ? cw("weekly", usedW, "unconfirmed", null) : cw("weekly", usedW, "confirmed", 3 * 24 * HOUR + 2 * HOUR);
  const windows = hasData ? [five5, weeklyW] : [cw("five_hour", null, "unconfirmed", null), cw("weekly", null, "unconfirmed", null)];
  // Rules and tones read current windows only: a passed reset is no value at all.
  const five = hasData && five5.present && five5.resetSchedule !== "passed" ? five5.remainingPercent : null;
  const weekly = hasData ? weeklyW.remainingPercent : null;
  const reasons = claudeScenario === "low" ? ["weekly_remaining_30", "weekly_forecast_at_risk"] : claudeScenario === "critical" ? ["weekly_remaining_10", "weekly_forecast_at_risk"] : claudeScenario === "stale" ? ["weekly_remaining_30"] : [];
  const card: ProviderDashboard = {
    provider: "claude",
    displayName: "Claude",
    planType: "max",
    cliVersion: "2.1.278",
    appServerVersion: null,
    compatibilityMode: false,
    health: { state: state === "live" ? "ok" : state, consecutiveFailures: 0, lastError: null },
    usage: {
      provider: "claude",
      capturedAtMs: hasData ? lastUpdate : null,
      availability: "unknown",
      ordinaryUsageAllowed: null,
      windows,
      resetCreditCount: null,
      bucketIds: [],
      account: "e5f6a7b8",
      polls: { snapshots: 0, pollRuns: 0, failedPolls: 0, lastStatus: null, lastError: null, lastPollAtMs: null },
      classifierVersion: "claude-observer-v1",
    },
    forecasts: hasData ? [forecast("five_hour", {}), forecast("weekly", { status: "ok", burnRatePerHour: 0.4, burnRatePerDay: 9.6, currentUsedPercent: usedW, remainingPercent: 100 - usedW, serverResetAtMs: now + 74 * HOUR, resetSchedule: "confirmed", estimatedExhaustionAtMs: now + ((100 - usedW) / 0.4) * HOUR, marginHours: (100 - usedW) / 0.4 - 74, pressure: (0.4 * 74) / (100 - usedW), risk: atRisk ? "at_risk" : "low", confidence: "Medium", burn: { status: "consuming", recent6hRate: 0.5, recent24hRate: 0.4, currentCycleRate: 0.38, points: 40, spanMs: 30 * HOUR, observedConsumptionPp: 12, latestUsedPercent: usedW, latestCapturedAtMs: lastUpdate } })] : [],
    extraLimits: hasData ? [{ key: "weekly_model:fable", label: "Fable", usedPercent: 3, remainingPercent: 97, resetsAtMs: now + 74 * HOUR, resetPrecisionMs: precision, current: true }] : [],
    refreshCapability: integrationOn ? "passive" : "unavailable",
    source: {
      state,
      lastSource: !hasData ? null : claudeScenario === "approx" ? "active_cli" : "statusline",
      lastUpdateMs: hasData ? lastUpdate : null,
      lastPassiveMs: hasData ? lastUpdate : null,
      lastActiveMs: null,
      live: state === "live",
      integrationEnabled: integrationOn,
      activeEnabled: settings.claude.activeRefresh ?? true,
      activeSuspended: false,
      activeNeedsVerification: false,
      activeNextAllowedMs: null,
      billing: state === "api_billing" ? "api_key" : "subscription",
      cliFound: state !== "not_found",
    },
  };
  const line: ProviderTrayLine = {
    provider: "claude",
    displayName: "Claude",
    severity: claudeScenario === "critical" ? "critical" : claudeScenario === "low" || claudeScenario === "stale" ? "warning" : "normal",
    reasons,
    fiveHourRemaining: five,
    weeklyRemaining: weekly,
    fiveHourTone: mockTone(five, state === "live"),
    weeklyTone: mockTone(weekly, state === "live"),
    live: state === "live",
    hasData,
    note: state === "live" ? null : state,
  };
  return { card, line };
}

/** ?history=partial: only the last 5 hours were recorded, with a 40-minute gap (QuotaDock not running). */
function partialHistory(points: HistoryPoint[], now: number): HistoryPoint[] {
  if (params.get("history") !== "partial") return points;
  const kept = points.filter((p) => p.t >= now - 5 * HOUR && (p.t < now - 3 * HOUR || p.t > now - 3 * HOUR + 40 * MIN));
  return kept.map((p, i) => ({ ...p, gapBefore: i > 0 && p.t - kept[i - 1]!.t > 30 * MIN }));
}

function history(range: HistoryRange): HistoryData {
  const now = Date.now();
  const span = range === "six_hours" ? 6 * HOUR : range === "twelve_hours" ? 12 * HOUR : 24 * HOUR;
  const five: HistoryPoint[] = [];
  const weekly: HistoryPoint[] = [];
  const rolloverAt = now - 4 * HOUR;
  for (let t = now - 24 * HOUR; t <= now; t += 5 * MIN) {
    const before = t < rolloverAt;
    const usedFive = before ? Math.min(96, 30 + ((t - (now - 24 * HOUR)) / HOUR) * 4.4) : Math.min(40, ((t - rolloverAt) / HOUR) * 1.4);
    five.push({ t, used: Math.round(usedFive), remaining: 100 - Math.round(usedFive), segment: before ? 1 : 2, gapBefore: false });
    const usedW = Math.round(6 + ((t - (now - 24 * HOUR)) / HOUR) * 0.5);
    weekly.push({ t, used: usedW, remaining: 100 - usedW, segment: 1, gapBefore: false });
  }
  const from = now - span;
  const keep = (pts: HistoryPoint[]) => partialHistory(pts, now).filter((p) => p.t >= from);
  return {
    range,
    fromMs: from,
    toMs: now,
    resolutionMs: null,
    series: [
      { provider: "codex", windowType: "five_hour", points: keep(five) },
      { provider: "codex", windowType: "weekly", points: keep(weekly) },
      ...(settings.claude.enabled
        ? [
            { provider: "claude" as const, windowType: "five_hour" as const, points: keep(five.filter((_, i) => i % 3 === 0).map((p) => ({ ...p, used: Math.max(0, p.used - 20), remaining: Math.min(100, p.remaining + 20) }))) },
            { provider: "claude" as const, windowType: "weekly" as const, points: keep(weekly.filter((_, i) => i % 3 === 0).map((p) => ({ ...p, used: p.used + 18, remaining: p.remaining - 18 }))) },
          ]
        : []),
    ],
    markers: [{ provider: "codex" as const, t: rolloverAt, windowType: "five_hour", eventType: "natural_rollover_observed", confidence: "high" }].filter((m) => m.t >= from),
    sourceSnapshots: five.length,
  };
}

function events(includeDiagnostic: boolean): EventView[] {
  const now = Date.now();
  const all: EventView[] = [
    {
      id: 6,
      provider: "codex",
      eventKey: "e6",
      windowType: "five_hour",
      eventType: "sliding_anchor_detected",
      visibility: "diagnostic",
      observedAtMs: now - 8 * HOUR - 50 * MIN,
      confidence: "medium",
      confirmed: true,
      evidence: { samples: 3, expectedDurationSeconds: 18000, offsetVarianceSeconds: 0.05, detected: true },
      supersededByEventKey: null,
    },
    {
      id: 5,
      provider: "codex",
      eventKey: "e5",
      windowType: "five_hour",
      eventType: "natural_rollover_observed",
      visibility: "user",
      observedAtMs: now - 9 * HOUR,
      confidence: "medium",
      confirmed: true,
      evidence: { usedPercentBefore: 12, usedPercentAfter: 0, resetAtBefore: now - 9 * HOUR - 4 * MIN, resetAtAfter: now - 4 * HOUR, crossedPreviousResetTime: true, creditsBefore: 2, creditsAfter: 2, creditsDelta: 0, slidingAnchorDetected: true, stableNewResetAtPolls: 1 },
      supersededByEventKey: null,
    },
    {
      id: 4,
      provider: "codex",
      eventKey: "e4",
      windowType: "five_hour",
      eventType: "transient_backend_state",
      visibility: "diagnostic",
      observedAtMs: now - 20 * HOUR,
      confidence: "low",
      confirmed: true,
      evidence: { rejectedCandidateKind: "schedule_change", reason: "returned to the pre-change state (A->B->A)", resetAtDeltaSeconds: 600 },
      supersededByEventKey: null,
    },
    {
      id: 3,
      provider: "codex",
      eventKey: "e3",
      windowType: "weekly",
      eventType: "unscheduled_quota_refresh",
      visibility: "user",
      observedAtMs: now - 3 * 24 * HOUR,
      confidence: "medium",
      confirmed: true,
      evidence: { usedPercentBefore: 70, usedPercentAfter: 1, resetAtBefore: now, resetAtAfter: now + 2 * 24 * HOUR, crossedPreviousResetTime: false, creditsBefore: 2, creditsAfter: 2, creditsDelta: 0, cause: "unknown", stableNewResetAtPolls: 3 },
      supersededByEventKey: null,
    },
  ];
  if (settings.claude.enabled) {
    all.unshift({ id: -1, provider: "claude", eventKey: "c1", windowType: "five_hour", eventType: "quota_refreshed_observed", visibility: "user", observedAtMs: now - 2 * HOUR, confidence: "medium", confirmed: true, evidence: { usedBefore: 81, usedAfter: 3, previousResetsAtMs: now - 2 * HOUR - 6 * MIN, newResetsAtMs: now + 3 * HOUR, note: "previous reset time had passed; cause not inferred" }, supersededByEventKey: null });
  }
  return all.filter((e) => includeDiagnostic || e.visibility === "user");
}

function settingsDto(): SettingsDto {
  return { settings, effectiveLanguage: effective(), dataFolder: "C:\\Users\\you\\AppData\\Local\\QuotaDock" };
}

/** Codex turned off in Settings: the backend leaves it out of every view. */
function withoutCodex(d: DashboardDto): DashboardDto {
  if (settings.codex.enabled) return d;
  const claude = d.tray.providers?.find((p) => p.provider === "claude");
  return {
    ...d,
    providers: d.providers.filter((p) => p.provider !== "codex"),
    tray: { ...d.tray, severity: claude?.severity ?? "normal", reasons: [], providers: claude ? [claude] : [], updatedAtMs: claude ? Date.now() - 2 * MIN : null, freshness: claude ? "fresh" : "no_data" },
  };
}

/** A hidden 5-hour limit: the backend takes it out of the page's data for that provider. */
function fiveShown(provider: string): boolean {
  return provider === "codex" ? settings.codex.showFiveHour : settings.claude.showFiveHour;
}

function withoutHiddenFive(d: DashboardDto): DashboardDto {
  return {
    ...d,
    providers: d.providers.map((p) => (fiveShown(p.provider) ? p : { ...p, usage: { ...p.usage, windows: p.usage.windows.filter((w) => w.windowType !== "five_hour") }, forecasts: p.forecasts.filter((f) => f.windowType !== "five_hour") })),
  };
}

export const mockBackend: Backend = {
  getDashboard: async () => withoutHiddenFive(withoutCodex(dashboard())),
  refreshUsage: async () => (paused ? "paused" : "started"),
  getUsageHistory: async (range) => {
    const h = history(range);
    const shown = settings.codex.enabled ? h : { ...h, series: h.series.filter((s) => s.provider !== "codex"), markers: [] };
    return { ...shown, series: shown.series.filter((s) => s.windowType !== "five_hour" || fiveShown(s.provider)), markers: shown.markers.filter((m) => m.windowType !== "five_hour" || fiveShown(m.provider)) };
  },
  getQuotaEvents: async (includeDiagnostic, beforeMs) => (beforeMs ? [] : events(includeDiagnostic).filter((e) => (settings.codex.enabled || e.provider !== "codex") && (e.windowType !== "five_hour" || fiveShown(e.provider)))),
  pauseMonitoring: async () => {
    paused = true;
  },
  resumeMonitoring: async () => {
    paused = false;
  },
  getSettings: async () => settingsDto(),
  updateSettings: async (s) => {
    settings = s;
    return settingsDto();
  },
  acknowledgeCloseHint: async () => {},
  openDataFolder: async () => {},
  exportDiagnostics: async () => "C:\\Users\\you\\AppData\\Local\\QuotaDock\\exports\\diagnostics-20261008-104200.json",
  quitApp: async () => {},
  getClaudeIntegration: async () => integrationView(),
  enableClaudeIntegration: async () => {
    integrationOn = true;
    return integrationView();
  },
  disableClaudeIntegration: async () => {
    integrationOn = false;
    return "restored";
  },
};

function integrationView(): ClaudeIntegrationView {
  const settingsPath = "C:\\Users\\you\\.claude\\settings.json";
  const statusLine = { type: "command", command: "~/.quotadock/bin/quotadock-claude-bridge.exe" };
  return integrationOn
    ? { enabled: { enabled: true, installedAtMs: Date.now() - HOUR, settingsPath, previousStatusLine: null, installedStatusLine: statusLine, backupPath: null }, plan: null, planError: null }
    : {
        enabled: null,
        plan: {
          settingsPath,
          settingsExists: true,
          currentStatusLine: null,
          newStatusLine: statusLine,
          chainsExisting: false,
          bridgeInstallPath: "C:\\Users\\you\\.quotadock\\bin\\quotadock-claude-bridge.exe",
          backupPath: "C:\\Users\\you\\AppData\\Local\\QuotaDock\\claude\\claude-settings.backup-1791460000000.json",
          plannedAtMs: 1791460000000,
          planId: "0123456789abcdef0123456789abcdef",
        },
        planError: null,
      };
}
