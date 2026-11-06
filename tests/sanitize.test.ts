import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { maskIdentifier, sanitizeForLog, sanitizeString } from "../src/codex/sanitize.ts";

describe("sanitizeForLog", () => {
  it("redacts credentials and identifiers at any depth", () => {
    const input = {
      tokens: { access_token: "SECRET-A", refresh_token: "SECRET-R", id_token: "SECRET-I" },
      headers: { Authorization: "Bearer SECRET-B", Cookie: "SECRET-C=1" },
      account: { email: "x@example.com", accountId: "SECRET-ACCT-1", chatgptAccountId: "SECRET-ACCT-2", userId: "SECRET-U" },
      remoteControl: { installationId: "SECRET-INST", serverName: "SECRET-HOST" },
      apiKey: "sk-should-not-appear",
      nested: [{ password: "SECRET-P" }],
    };
    const out = JSON.stringify(sanitizeForLog(input));
    for (const secret of ["SECRET-", "x@example.com", "sk-should"]) {
      assert.equal(out.includes(secret), false, `${secret} leaked: ${out}`);
    }
  });

  it("keeps quota fields and harmless ids", () => {
    const input = { id: 3, limitId: "codex", windowDurationMins: 300, usedPercent: 12, resetsAt: 1791393584, planType: "plus" };
    assert.deepEqual(sanitizeForLog(input), input);
  });

  it("scrubs JWTs, bearer headers, API keys and e-mails inside free text", () => {
    const s = sanitizeString("auth=Bearer abc.def ghi eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIn0.sig mail me@x.io key sk-1234567890abcdef");
    assert.equal(/abc\.def|eyJhbGci|me@x\.io|sk-1234567890/.test(s), false, s);
  });

  it("leaves null values alone", () => {
    assert.deepEqual(sanitizeForLog({ accountId: null }), { accountId: null });
  });
});

describe("maskIdentifier", () => {
  it("keeps a short prefix and suffix", () => {
    assert.equal(maskIdentifier("47821234-aaaa-bbbb-cccc-1234567890b0"), "4782****b0");
    assert.equal(maskIdentifier("short"), "****");
    assert.equal(maskIdentifier(null), null);
  });
});
