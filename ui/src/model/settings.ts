// How the Settings page changes its copy: the whole object is saved, with only the touched fields
// different. Nothing here forces, migrates or rewrites a field the control does not own.

import type { AppSettings, ClaudeNotificationSettings, ClaudeSettings, CodexSettings, DesktopWidgetSettings, NotificationSettings } from "../api/types.ts";

export const withApp = (s: AppSettings, patch: Partial<AppSettings>): AppSettings => ({ ...s, ...patch });
export const withCodex = (s: AppSettings, patch: Partial<CodexSettings>): AppSettings => ({ ...s, codex: { ...s.codex, ...patch } });
export const withClaude = (s: AppSettings, patch: Partial<ClaudeSettings>): AppSettings => ({ ...s, claude: { ...s.claude, ...patch } });
export const withWidget = (s: AppSettings, patch: Partial<DesktopWidgetSettings>): AppSettings => ({ ...s, desktopWidget: { ...s.desktopWidget, ...patch } });
export const withNotifications = (s: AppSettings, patch: Partial<NotificationSettings>): AppSettings => ({ ...s, notifications: { ...s.notifications, ...patch } });
export const withClaudeNotifications = (s: AppSettings, patch: Partial<ClaudeNotificationSettings>): AppSettings => ({
  ...s,
  notifications: { ...s.notifications, claude: { ...s.notifications.claude, ...patch } },
});

/**
 * The page's saves, one `update_settings` at a time and in order. Each copy is whole and built on the one
 * before, so of those still waiting only the newest needs sending. Never two at once: the backend handles
 * each call on its own task, so an older copy could be stored after a newer one and undo it. Only the newest
 * save's reply counts, and a refused newest save reads the stored settings back. A read asked for during saves
 * waits for them, because it would otherwise put an older copy under the next click.
 */
export function settingsQueue<R>(io: { send: (next: AppSettings) => Promise<R>; read: () => Promise<R>; apply: (reply: R) => void }) {
  let waiting: AppSettings | null = null;
  let running = false;
  let settled: Promise<void> = Promise.resolve();
  let readAfter = false;
  let newest = 0; // a read still out when something newer starts is out of date

  const read = async (): Promise<void> => {
    if (running) {
      readAfter = true;
      return;
    }
    const mine = ++newest;
    try {
      const reply = await io.read();
      if (mine === newest) io.apply(reply);
    } catch {
      // Nothing better to show than the copy already on the page.
    }
  };
  const drain = async (): Promise<void> => {
    while (waiting !== null) {
      const next: AppSettings = waiting;
      waiting = null;
      try {
        const reply = await io.send(next);
        if (waiting === null) io.apply(reply);
      } catch {
        if (waiting === null) readAfter = true;
      }
    }
    running = false;
    if (readAfter) {
      readAfter = false;
      await read();
    }
  };
  return {
    /** Settles once every save so far has finished; a refusal shows as the re-read, not as a rejection. */
    save(next: AppSettings): Promise<void> {
      newest++;
      waiting = next;
      if (!running) {
        running = true;
        settled = drain();
      }
      return settled;
    },
    read,
  };
}

/** Reminders switched on among the switches `调整提醒` shows: enabled providers and shown windows only. */
export function enabledReminderCount(s: AppSettings): number {
  const n = s.notifications;
  const c = n.claude;
  const codex = s.codex.enabled
    ? [n.weeklyQuota, s.codex.showFiveHour && n.fiveHourQuota, n.forecastWeekly, s.codex.showFiveHour && n.forecastFiveHour, n.availabilityRestored, n.quotaRefresh, n.resetCredit, n.allowanceRefresh, n.providerErrors]
    : [];
  const claude = s.claude.enabled ? [c.weeklyQuota, s.claude.showFiveHour && c.fiveHourQuota, c.forecastWeekly, c.allowanceRefresh] : [];
  return [...codex, ...claude].filter(Boolean).length;
}
