# Claude Data Source Findings（Phase 3.5 可行性门禁）

QuotaDock Phase 3.5 · 2026-10-08 · Windows 11 · Claude Code 2.1.278

依据标记：

| 标记 | 含义 |
|---|---|
| `[LOCAL TEST]` | 本机实测 |
| `[ANTHROPIC DOCS]` | Anthropic 官方文档（code.claude.com/docs） |
| `[CLAUDE CODE OBSERVED]` | 本机 Claude Code 的实际输出 |
| `[CLAUDE CODE ISSUE]` | 社区 issue，**不是**官方契约 |
| `[QUOTADOCK HEURISTIC]` | QuotaDock 自己定的规则 |
| `[UNKNOWN]` | 未知 |

## 结论

> **Can we read Claude subscription quota safely? — YES**

找到两条安全路径，都不需要读取凭据，也都不消耗推理额度：

| 数据源 | 类型 | 证据 |
|---|---|---|
| Claude Code statusLine 的 `rate_limits` | 被动推送，结构化 JSON，**主数据源** | `[ANTHROPIC DOCS]` 字段已在文档中写明；本机实测需要在用户授权修改 settings 之后进行 |
| `claude -p /usage --output-format json` | 主动查询，文本，**默认关闭的补充数据源** | `[LOCAL TEST]` 确认是本地 slash command，不调用模型 |

两条路径都**不**读取 `~/.claude/.credentials.json`，**不**由 QuotaDock 自己请求 `api.anthropic.com/api/oauth/usage`，**不**发送 prompt。

## 环境 `[LOCAL TEST]`

- **Claude Code CLI 2.1.278**：npm 全局安装，入口是 `%APPDATA%\npm\claude.cmd`，实际二进制是 `%APPDATA%\npm\node_modules\@anthropic-ai\claude-code\bin\claude.exe`。
- **Claude Desktop**：正在运行（本次工作会话就在它的 Code 标签中）。
- **VS Code 插件**：未检查。
- **`claude --help`**：没有 usage 子命令；有 `auth status`、`-p/--print`、`--output-format json`、`--no-session-persistence`、`--model`、`--settings`。

## 认证方式 `[LOCAL TEST]`

`claude auth status --json` 输出的非个人字段：

```text
loggedIn = true
authMethod = claude.ai
apiProvider = firstParty
subscriptionType = max
```

`email`、`orgId`、`orgName` 也在输出中。QuotaDock 不记录、不显示这些值，只在内存中把它们哈希成 fingerprint。

`ANTHROPIC_API_KEY`、`ANTHROPIC_AUTH_TOKEN`、`CLAUDE_CODE_USE_BEDROCK/VERTEX/FOUNDRY`、`CLAUDE_CODE_OAUTH_TOKEN` 在用户和系统环境中**都不存在**。检测方式是只看变量是否存在，不读取值。结论：当前走订阅计费，没有 API key 覆盖。

注意：在 Claude Desktop 内运行的进程会继承大量注入的变量（`CLAUDE_CODE_*`、`ANTHROPIC_BASE_URL`、消息 socket/token 等）。QuotaDock 启动 claude 子进程时**必须**清除这些变量，只使用用户级和系统级环境，否则测到的是 "Desktop 内嵌会话"，而不是用户自己的 Claude Code。

## 数据源调查

### Priority 1：CLI 机器可读输出 `[LOCAL TEST]`

**测试方法（零额度）**：

```text
claude -p /usage --output-format json --model qd-nonexistent-model --no-session-persistence --strict-mcp-config
```

- 在干净环境、全新空目录中运行。
- 故意使用**不存在的模型名**：如果 `/usage` 被当作 prompt 发给模型，请求会因模型不存在而失败，不可能产生推理；如果它是本地命令，模型名根本不会被使用。

**结果**（两次一致）：

| 字段 | 值 |
|---|---|
| `type` / `subtype` / `is_error` | `result` / `success` / `false` |
| `local_command` | `"usage"` |
| `duration_api_ms` | **0** |
| `num_turns` | **0** |
| `total_cost_usd` | **0** |
| `usage.*_tokens` | 全部 **0** |
| `modelUsage` | 空 |
| 耗时 | 冷启动 8.5 s，热启动 2.2 s |
| 残留进程 | 无 |
| 会话文件 | 无（`--no-session-persistence` 生效）；Claude Code 为该工作目录建了一个空的 `memory\` 目录 |

结论：`/usage` 在 `-p` 模式下由 Claude Code **本地拦截执行**，不调用模型，不消耗推理额度。

GPT 授权的那次 "可能消耗少量额度的一次性验证" **没有执行**，因为零成本方法已经给出结论。

**输出格式**（`result` 字段，节选，个人信息已去除）：

```text
You are currently using your subscription to power your Claude Code usage

Current session: 6% used · resets Oct 9, 1:19am (Asia/Seoul)
Current week (all models): 29% used · resets Oct 12, 11:59pm (Asia/Seoul)
Current week (Fable): 1% used · resets Oct 12, 11:59pm (Asia/Seoul)

What's contributing to your limits usage?
…（基于本机会话的近似统计，QuotaDock 不使用）
```

`[INFERENCE]` 底层很可能是 Claude Code 用它自己的凭据请求 usage 端点。所以这条路径仍可能受服务端限流影响（`[CLAUDE CODE ISSUE]` 有 429 报告），按 GPT 的决定只能作为**默认关闭的过期数据兜底**。

### Priority 2：statusLine `[ANTHROPIC DOCS]`

文档 `code.claude.com/docs/en/statusline` 明确列出：

| 字段 | 含义 |
|---|---|
| `rate_limits.five_hour.used_percentage` | 5 小时窗口已用 %，0–100，可带小数 |
| `rate_limits.five_hour.resets_at` | Unix 秒 |
| `rate_limits.seven_day.used_percentage` / `resets_at` | 7 天窗口 |
| `rate_limits.spend_limit.*` | 只出现在企业网关场景，与本账号无关 |

限制条件：

- 只对 claude.ai Pro/Max 订阅用户提供。
- 会话中第一次 API 响应之后才出现。
- 每个窗口可能单独缺失；过了 `resets_at` 后 Claude Code 会丢弃该窗口。
- 事件驱动更新（有新消息等事件时），另可设置 `refreshInterval`；debounce 300 ms。
- Windows 上通过 Git Bash 执行（没有 Git Bash 时用 PowerShell），命令路径要用正斜杠。
- 配置自定义 statusLine 后，Claude Code 会隐藏底部大部分快捷键提示。
- 在 Desktop Code 标签和 IDE 插件中是否执行：`[UNKNOWN]`，需要启用后实测。

本机 `~/.claude/settings.json` 目前**没有** `statusLine`（只检查了键名）。

### Priority 3 / 4

不需要。没有检查浏览器 cookie，没有读取凭据文件，没有使用 OCR 或 UI 自动化。

## 字段证据表

| Field | statusLine | /usage（-p） |
|---|---|---|
| 5h used % | YES（小数） | YES（整数） |
| 5h remaining % | derived | derived |
| 5h reset time | YES（秒） | YES（分钟精度，本地时区文本） |
| weekly used % | YES | YES |
| weekly remaining % | derived | derived |
| weekly reset time | YES | YES |
| extra model limits | NO | YES（`Current week (Fable)`） |
| availability authority | NO | NO |
| subscription plan | NO（用 `claude auth status`） | 只说明 "using your subscription" |
| source live | conditional（只在 Claude Code 会话中推送） | YES（主动查询时） |

## 选定方案

1. **主数据源**：statusLine 桥接，默认开启。需要用户在 QuotaDock 中显式 "启用 Claude 集成"，事先展示改动、备份、保留已有 statusLine；关闭时恢复。
2. **补充数据源**：`/usage` 主动查询，默认关闭。
   - 只在没有新鲜被动数据时运行：前台超过 5 分钟、后台超过 15 分钟没有被动数据。
   - 遇到 429 时遵守 `Retry-After`，否则按 15 → 30 → 60 分钟退避；连续 3 次后本次会话暂停主动刷新。
   - 永远带一个**故意无效的模型名**作为保险：万一将来的 Claude Code 版本把 `/usage` 改成调用模型，请求会失败而不是消耗额度。
   - 每次检查 JSON 外壳：`local_command == "usage"`、`duration_api_ms == 0`、token 都为 0，否则立即停用。
3. **认证方式检测**：`claude auth status --json`，加上 API key 环境变量是否存在（只看是否存在）。
4. **availability**：Unknown。百分比到 100 不推断为不可用。
