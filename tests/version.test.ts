import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { extractDaemonFields, selectInstallation, type DiscoveredInstallation } from "../src/codex/discovery.ts";
import { compareVersions, gateVersion, parseCodexVersionOutput } from "../src/codex/version.ts";

describe("versions", () => {
  it("parses `codex --version` output", () => {
    assert.equal(parseCodexVersionOutput("codex-cli 0.162.0-alpha.2\n"), "0.162.0-alpha.2");
    assert.equal(parseCodexVersionOutput("codex-cli 0.154.0-alpha.6.2"), "0.154.0-alpha.6.2");
    assert.equal(parseCodexVersionOutput("something else"), null);
  });

  it("orders pre-releases like semver", () => {
    const ordered = ["0.154.0-alpha.6.2", "0.160.0", "0.162.0-alpha.2", "0.162.0-alpha.10", "0.162.0", "0.163.0-alpha.1"];
    for (let i = 0; i < ordered.length - 1; i++) {
      assert.equal(compareVersions(ordered[i]!, ordered[i + 1]!), -1, `${ordered[i]} < ${ordered[i + 1]}`);
      assert.equal(compareVersions(ordered[i + 1]!, ordered[i]!), 1);
    }
    assert.equal(compareVersions("0.162.0-alpha.2", "0.162.0-alpha.2"), 0);
  });

  it("gates against the verified version", () => {
    assert.equal(gateVersion("0.162.0-alpha.2"), "verified");
    assert.equal(gateVersion("0.163.0"), "unverified-newer");
    assert.equal(gateVersion("0.154.0-alpha.6.2"), "unsupported-older");
    assert.equal(gateVersion("banana"), "unparseable");
  });
});

describe("selectInstallation", () => {
  const desktop: DiscoveredInstallation = { path: "C:\\desktop\\codex.exe", version: "0.162.0-alpha.2", source: "desktop", gate: "verified" };
  const runtime: DiscoveredInstallation = { path: "C:\\runtime\\codex.exe", version: "0.162.0-alpha.2", source: "desktop", gate: "verified" };
  const vscode: DiscoveredInstallation = { path: "C:\\vscode\\codex.exe", version: "0.154.0-alpha.6.2", source: "vscode", gate: "unsupported-older" };

  it("prefers 0.162.0-alpha.2 over the older VS Code copy", () => {
    assert.equal(selectInstallation([vscode, desktop, runtime]).selected?.path, desktop.path);
  });

  it("never picks an older version unless explicitly allowed", () => {
    assert.equal(selectInstallation([vscode]).selected, null);
    assert.equal(selectInstallation([vscode], true).selected?.path, vscode.path);
  });

  it("picks a newer standalone CLI over the desktop copy", () => {
    const standalone: DiscoveredInstallation = { path: "C:\\standalone\\codex.exe", version: "0.163.0", source: "standalone", gate: "unverified-newer" };
    assert.equal(selectInstallation([desktop, standalone]).selected?.path, standalone.path);
  });

  it("honours a manually configured executable", () => {
    const manual: DiscoveredInstallation = { ...desktop, path: "D:\\custom\\codex.exe", source: "manual" };
    assert.equal(selectInstallation([desktop, manual]).selected?.path, manual.path);
  });
});

describe("extractDaemonFields", () => {
  it("reads the documented camelCase lifecycle fields", () => {
    assert.deepEqual(extractDaemonFields({ status: "running", backend: "pid", socketPath: "x", cliVersion: "0.162.0", appServerVersion: "0.162.0" }), { version: "0.162.0", backend: "pid" });
    assert.deepEqual(extractDaemonFields(null), {});
  });
});
