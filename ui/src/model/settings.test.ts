import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import type { AppSettings, ClaudeNotificationSettings, HealthState, NotificationSettings } from "../api/types.ts";
import { en } from "../i18n/en.ts";
import { lookup } from "../i18n/index.ts";
import { zhCN } from "../i18n/zh-CN.ts";
import { enabledReminderCount, settingsQueue, withApp, withClaude, withClaudeNotifications, withCodex, withNotifications, withWidget } from "./settings.ts";

const allOff: NotificationSettings = {
  weeklyQuota: false,
  fiveHourQuota: false,
  forecastWeekly: false,
  forecastFiveHour: false,
  availabilityRestored: false,
  quotaRefresh: false,
  resetCredit: false,
  allowanceRefresh: false,
  providerErrors: false,
  claude: { weeklyQuota: false, fiveHourQuota: false, forecastWeekly: false, allowanceRefresh: false },
};
const allOn: NotificationSettings = {
  weeklyQuota: true,
  fiveHourQuota: true,
  forecastWeekly: true,
  forecastFiveHour: true,
  availabilityRestored: true,
  quotaRefresh: true,
  resetCredit: true,
  allowanceRefresh: true,
  providerErrors: true,
  claude: { weeklyQuota: true, fiveHourQuota: true, forecastWeekly: true, allowanceRefresh: true },
};

function settings(notifications: NotificationSettings = allOff): AppSettings {
  return {
    language: "system",
    theme: "system",
    closeToTray: true,
    closeHintDismissed: false,
    launchAtStartup: false,
    showDiagnostics: false,
    notifications,
    codex: { enabled: true, showFiveHour: true },
    claude: { enabled: true, showFiveHour: true },
    desktopWidget: { enabled: false, alwaysOnTop: true, opacity: 65, mode: "compact", rememberPosition: true, locked: false },
  };
}

describe("enabled reminder count", () => {
  it("counts every switch that is on, none when all are off", () => {
    assert.equal(enabledReminderCount(settings(allOff)), 0);
    assert.equal(enabledReminderCount(settings(allOn)), 13);
  });

  it("counts each Codex rule and each Claude rule once", () => {
    for (const key of Object.keys(allOff).filter((k) => k !== "claude") as (keyof Omit<NotificationSettings, "claude">)[]) {
      assert.equal(enabledReminderCount(settings({ ...allOff, [key]: true })), 1, key);
    }
    for (const key of Object.keys(allOff.claude) as (keyof ClaudeNotificationSettings)[]) {
      assert.equal(enabledReminderCount(settings({ ...allOff, claude: { ...allOff.claude, [key]: true } })), 1, `claude ${key}`);
    }
  });

  it("leaves out a provider that is off", () => {
    assert.equal(enabledReminderCount(withCodex(settings(allOn), { enabled: false })), 4);
    assert.equal(enabledReminderCount(withClaude(settings(allOn), { enabled: false })), 9);
    assert.equal(enabledReminderCount(withClaude(withCodex(settings(allOn), { enabled: false }), { enabled: false })), 0);
  });

  it("leaves out the rules of a hidden 5-hour window, and only those", () => {
    assert.equal(enabledReminderCount(withCodex(settings(allOn), { showFiveHour: false })), 11);
    assert.equal(enabledReminderCount(withClaude(settings(allOn), { showFiveHour: false })), 12);
    assert.equal(enabledReminderCount(withClaude(withCodex(settings(allOn), { showFiveHour: false }), { showFiveHour: false })), 10);
  });

  it("does not depend on a 5-hour rule stored on while its window is hidden", () => {
    const stored = { ...allOff, fiveHourQuota: true, forecastFiveHour: true, claude: { ...allOff.claude, fiveHourQuota: true } };
    assert.equal(enabledReminderCount(settings(stored)), 3);
    assert.equal(enabledReminderCount(withClaude(withCodex(settings(stored), { showFiveHour: false }), { showFiveHour: false })), 0);
  });
});

// Every write on the page that is not a reminder switch, one per control.
const otherWrites: [string, (s: AppSettings) => AppSettings][] = [
  ["launchAtStartup", (s) => withApp(s, { launchAtStartup: true })],
  ["language", (s) => withApp(s, { language: "zh-CN" })],
  ["theme", (s) => withApp(s, { theme: "dark" })],
  ["closeToTray", (s) => withApp(s, { closeToTray: false })],
  ["showDiagnostics", (s) => withApp(s, { showDiagnostics: true })],
  ["codex.enabled off", (s) => withCodex(s, { enabled: false })],
  ["codex.enabled on", (s) => withCodex(s, { enabled: true })],
  ["codex.showFiveHour off", (s) => withCodex(s, { showFiveHour: false })],
  ["codex.showFiveHour on", (s) => withCodex(s, { showFiveHour: true })],
  ["claude.enabled off", (s) => withClaude(s, { enabled: false })],
  ["claude.enabled on", (s) => withClaude(s, { enabled: true })],
  ["claude.showFiveHour off", (s) => withClaude(s, { showFiveHour: false })],
  ["claude.showFiveHour on", (s) => withClaude(s, { showFiveHour: true })],
  ["claude.activeRefresh", (s) => withClaude(s, { activeRefresh: false })],
  ["desktopWidget.enabled", (s) => withWidget(s, { enabled: true })],
  ["desktopWidget.opacity", (s) => withWidget(s, { opacity: 90 })],
  ["desktopWidget.alwaysOnTop", (s) => withWidget(s, { alwaysOnTop: false })],
  ["desktopWidget.mode", (s) => withWidget(s, { mode: "detailed" })],
  ["desktopWidget.rememberPosition", (s) => withWidget(s, { rememberPosition: false })],
  ["desktopWidget.locked", (s) => withWidget(s, { locked: true })],
];

describe("saving the whole settings copy", () => {
  // Stored values no control would produce together, and a field this version does not know.
  const stored = { ...allOff, fiveHourQuota: true, forecastFiveHour: true, resetCredit: true, futureRule: "kept", claude: { ...allOff.claude, fiveHourQuota: true, allowanceRefresh: true } } as NotificationSettings;

  it("leaves every notification field byte-identical on any other setting", () => {
    for (const notifications of [allOff, allOn, stored]) {
      const before = settings(notifications);
      const text = JSON.stringify(before.notifications);
      for (const [name, write] of otherWrites) {
        const after = write(before);
        assert.equal(JSON.stringify(after.notifications), text, name);
        assert.equal(after.notifications, before.notifications, `${name} shares the same object`);
      }
    }
  });

  it("still does after several other settings in a row", () => {
    const before = settings(stored);
    const after = otherWrites.reduce((s, [, write]) => write(s), before);
    assert.equal(JSON.stringify(after.notifications), JSON.stringify(before.notifications));
  });

  it("does not switch a rule off when its window is hidden or its provider is off", () => {
    const before = settings(allOn);
    const after = withClaude(withCodex(before, { enabled: false, showFiveHour: false }), { enabled: false, showFiveHour: false });
    assert.deepEqual(after.notifications, allOn);
  });

  it("changes exactly one field for each reminder switch", () => {
    const before = settings(allOff);
    for (const key of Object.keys(allOff).filter((k) => k !== "claude") as (keyof Omit<NotificationSettings, "claude">)[]) {
      assert.deepEqual(withNotifications(before, { [key]: true }), { ...before, notifications: { ...allOff, [key]: true } }, key);
    }
    for (const key of Object.keys(allOff.claude) as (keyof ClaudeNotificationSettings)[]) {
      assert.deepEqual(withClaudeNotifications(before, { [key]: true }), { ...before, notifications: { ...allOff, claude: { ...allOff.claude, [key]: true } } }, `claude ${key}`);
    }
  });

  it("does not change the page's copy in place", () => {
    const before = settings(stored);
    const text = JSON.stringify(before);
    withNotifications(before, { weeklyQuota: true });
    withClaudeNotifications(before, { weeklyQuota: true });
    for (const [, write] of otherWrites) write(before);
    assert.equal(JSON.stringify(before), text);
  });
});

describe("overlapping saves", () => {
  type Reply = { from: string; settings: AppSettings };
  /** A backend whose calls stay open until the test ends them, and the page's copy with its optimistic update. */
  function page(start = settings(allOff)) {
    const calls: { settings: AppSettings; ok: (from: string) => void; fail: () => void }[] = [];
    const reads: { ok: (r: Reply) => void; fail: () => void }[] = [];
    let open = 0;
    let most = 0;
    const applied: Reply[] = [];
    const state = { copy: start };
    const queue = settingsQueue<Reply>({
      send: (next) =>
        new Promise<Reply>((resolve, reject) => {
          most = Math.max(most, ++open);
          calls.push({
            settings: next,
            ok: (from) => {
              open--;
              resolve({ from, settings: next });
            },
            fail: () => {
              open--;
              reject(new Error("refused"));
            },
          });
        }),
      read: () => new Promise<Reply>((resolve, reject) => reads.push({ ok: resolve, fail: () => reject(new Error("no")) })),
      apply: (reply) => {
        applied.push(reply);
        state.copy = reply.settings;
      },
    });
    // What saveSettings does: the state moves first, so the next click builds on this copy.
    const click = (write: (s: AppSettings) => AppSettings) => {
      state.copy = write(state.copy);
      return queue.save(state.copy);
    };
    return { calls, reads, applied, state, queue, click, most: () => most };
  }
  const tick = () => new Promise((r) => setTimeout(r));

  it("a slow first save and a quick second: in order, the reminder kept, the older reply ignored", async () => {
    const p = page();
    const done = p.click((s) => withCodex(s, { enabled: false }));
    p.click((s) => withNotifications(s, { weeklyQuota: true }));
    await tick();
    assert.equal(p.calls.length, 1, "the second waits for the first");
    assert.equal(p.calls[0]!.settings.codex.enabled, false);
    assert.equal(p.state.copy.notifications.weeklyQuota, true, "the page keeps showing the newer copy");

    p.calls[0]!.ok("first");
    await tick();
    assert.equal(p.calls.length, 2);
    assert.equal(p.applied.length, 0, "the first reply is older than what the page shows");
    assert.equal(p.state.copy.notifications.weeklyQuota, true);
    assert.equal(p.calls[1]!.settings.codex.enabled, false, "the first change is still in the second copy");
    assert.equal(p.calls[1]!.settings.notifications.weeklyQuota, true);

    p.calls[1]!.ok("second");
    await done;
    assert.deepEqual(p.applied.map((r) => r.from), ["second"]);
    assert.equal(p.most(), 1);
  });

  it("the same in the other direction: a reminder first, then a slow provider switch", async () => {
    const p = page();
    const done = p.click((s) => withClaudeNotifications(s, { allowanceRefresh: true }));
    p.click((s) => withCodex(s, { enabled: false }));
    p.calls[0]!.ok("first");
    await tick();
    assert.equal(p.calls[1]!.settings.notifications.claude.allowanceRefresh, true);
    p.calls[1]!.ok("second");
    await done;
    assert.equal(p.state.copy.notifications.claude.allowanceRefresh, true);
    assert.equal(p.state.copy.codex.enabled, false);
  });

  it("off and on again: the on is sent only after the off has finished", async () => {
    const p = page();
    const done = p.click((s) => withCodex(s, { enabled: false }));
    p.click((s) => withCodex(s, { enabled: true }));
    await tick();
    assert.deepEqual(p.calls.map((c) => c.settings.codex.enabled), [false]);
    p.calls[0]!.ok("off");
    await tick();
    assert.deepEqual(p.calls.map((c) => c.settings.codex.enabled), [false, true]);
    p.calls[1]!.ok("on");
    await done;
    assert.equal(p.most(), 1);
    assert.equal(p.state.copy.codex.enabled, true);
  });

  it("of the copies waiting only the newest is sent, and it holds every change", async () => {
    const p = page();
    const done = p.click((s) => withCodex(s, { enabled: false }));
    p.click((s) => withApp(s, { theme: "dark" }));
    p.click((s) => withNotifications(s, { resetCredit: true }));
    p.calls[0]!.ok("first");
    await tick();
    assert.equal(p.calls.length, 2);
    const last = p.calls[1]!.settings;
    assert.deepEqual([last.codex.enabled, last.theme, last.notifications.resetCredit], [false, "dark", true]);
    p.calls[1]!.ok("last");
    await done;
    assert.deepEqual(p.applied.map((r) => r.from), ["last"]);
  });

  it("a click just after a reply starts a new save, and that reply still counts", async () => {
    const p = page();
    const first = p.click((s) => withApp(s, { theme: "dark" }));
    p.calls[0]!.ok("first");
    await first;
    assert.deepEqual(p.applied.map((r) => r.from), ["first"]);
    const second = p.click((s) => withApp(s, { theme: "light" }));
    assert.equal(p.calls.length, 2);
    p.calls[1]!.ok("second");
    await second;
    assert.deepEqual(p.applied.map((r) => r.from), ["first", "second"]);
  });

  it("a refused save reads the stored settings back, and does not reject", async () => {
    const p = page();
    const before = p.state.copy;
    const done = p.click((s) => withCodex(s, { enabled: false }));
    p.calls[0]!.fail();
    await tick();
    assert.equal(p.reads.length, 1);
    assert.equal(p.state.copy.codex.enabled, false, "still the optimistic copy until the read answers");
    p.reads[0]!.ok({ from: "stored", settings: before });
    await done;
    assert.equal(p.state.copy.codex.enabled, true, "the switch moves back");
  });

  it("an older save refused while a newer waits: the newer goes out, no read in between", async () => {
    const p = page();
    const done = p.click((s) => withCodex(s, { enabled: false }));
    p.click((s) => withNotifications(s, { weeklyQuota: true }));
    p.calls[0]!.fail();
    await tick();
    assert.equal(p.reads.length, 0);
    assert.equal(p.calls.length, 2);
    p.calls[1]!.ok("second");
    await done;
    assert.equal(p.reads.length, 0);
    assert.deepEqual(p.applied.map((r) => r.from), ["second"]);
  });

  it("an older save accepted and the newest refused: the read shows what was stored", async () => {
    const p = page();
    const done = p.click((s) => withCodex(s, { enabled: false }));
    const stored = p.state.copy;
    p.click((s) => withNotifications(s, { weeklyQuota: true }));
    p.calls[0]!.ok("first");
    await tick();
    p.calls[1]!.fail();
    await tick();
    assert.equal(p.reads.length, 1);
    p.reads[0]!.ok({ from: "stored", settings: stored });
    await done;
    assert.deepEqual(p.applied.map((r) => r.from), ["stored"]);
    assert.equal(p.state.copy.notifications.weeklyQuota, false);
  });

  it("a read that cannot answer leaves the copy alone and does not reject", async () => {
    const p = page();
    const done = p.click((s) => withCodex(s, { enabled: false }));
    p.calls[0]!.fail();
    await tick();
    p.reads[0]!.fail();
    await done;
    assert.deepEqual(p.applied, []);
  });

  it("a read asked for during a save waits for it", async () => {
    const p = page();
    const done = p.click((s) => withCodex(s, { enabled: false }));
    void p.queue.read();
    assert.equal(p.reads.length, 0);
    p.calls[0]!.ok("first");
    await tick();
    assert.equal(p.reads.length, 1, "after the save, so it sees the result");
    p.reads[0]!.ok({ from: "stored", settings: p.state.copy });
    await done;
    assert.deepEqual(p.applied.map((r) => r.from), ["first", "stored"]);
  });

  it("a read already out when a save starts cannot put the older copy back", async () => {
    const p = page();
    const before = p.state.copy;
    void p.queue.read();
    const done = p.click((s) => withCodex(s, { enabled: false }));
    p.reads[0]!.ok({ from: "old read", settings: before });
    await tick();
    assert.equal(p.state.copy.codex.enabled, false);
    p.calls[0]!.ok("save");
    await done;
    assert.deepEqual(p.applied.map((r) => r.from), ["save"]);
  });

  it("a backend call that throws at once does not stop later saves", async () => {
    let calls = 0;
    const applied: number[] = [];
    const queue = settingsQueue<number>({
      send: () => {
        if (++calls === 1) throw new Error("no");
        return Promise.resolve(calls);
      },
      read: () => Promise.resolve(0),
      apply: (r) => applied.push(r),
    });
    await queue.save(settings());
    await queue.save(settings());
    assert.deepEqual(applied, [0, 2]);
  });
});

describe("version shown in Settings", () => {
  it("is the app's own version", () => {
    const read = (path: string) => JSON.parse(readFileSync(new URL(path, import.meta.url), "utf8")) as { version: string };
    assert.equal(read("../../package.json").version, read("../../../src-tauri/tauri.conf.json").version);
  });
});

describe("status word in AI services", () => {
  it("has words for every provider state, in both languages", () => {
    const states: Exclude<HealthState, "ok">[] = ["connecting", "paused", "not_found", "not_authenticated", "unsupported_version", "reconnecting", "stale", "waiting", "not_configured", "api_billing", "source_error", "needs_verification", "disabled"];
    for (const state of states) {
      assert.notEqual(lookup(en, `state_${state}`, "none"), "none", state);
      assert.notEqual(lookup(zhCN, `state_${state}`, "none"), "none", state);
    }
  });
});

describe("reminder count wording", () => {
  it("reads naturally in both languages", () => {
    assert.equal(zhCN.nEnabledCount(7), "已开启 7 项提醒");
    assert.equal(en.nEnabledCount(7), "7 reminders on");
    assert.equal(en.nEnabledCount(1), "1 reminder on");
    assert.equal(en.nEnabledCount(0), "0 reminders on");
  });

  it("keeps the names the UI Automation checks look for", () => {
    assert.equal(zhCN.providerEnabled, "启用");
    assert.equal(zhCN.showFiveHour, "显示 5 小时额度");
    assert.equal(zhCN.quitApp, "退出 QuotaDock");
  });
});
