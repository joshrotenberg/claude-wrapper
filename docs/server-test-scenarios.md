# claude-server validation scenarios

Manual integration scenarios for the `server` feature. Run periodically while iterating to catch regressions in the nested-claude loop.

The setup assumes the project's `.mcp.json` is registered (or available via `--mcp-config`) and points at `cargo run --release --features server` so each test picks up the latest code.

---

## A. Basic plumbing

1. **`tools/list` returns N tools.** Currently 21. Increment when streaming/cli/agent surface grows.
   ```
   echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}'\
        '{"jsonrpc":"2.0","method":"notifications/initialized"}'\
        '{"jsonrpc":"2.0","id":2,"method":"tools/list"}'\
     | cargo run --release --features server --bin claude-server --quiet -- --config .claude-server-dev.toml 2>/dev/null | tail -1
   ```

2. **`claude.cli_version` returns structured `{major, minor, patch, display}`.** Cheapest end-to-end probe; no LLM cost.

3. **`claude.version` returns `CommandOutput` with stdout containing version string.**

---

## B. Single-turn agent

4. **`agent.ask` returns a numeric answer.**
   - Prompt: "What is 7 times 9? Just the number."
   - Expect: result text contains `63`, plus session_id and cost.

5. **`agent.ask` with model override.**
   - Same prompt, `model: "haiku"` in overrides. Verify cost is lower than the default-model run.

6. **`agent.ask` with `bare: true` override.** Verify it fails with auth error if the host uses keychain auth and no `ANTHROPIC_API_KEY` is set. Confirms the override path is wired.

---

## C. Multi-turn chat

7. **Chat continuity across turns.**
   - `agent.chat.open` -> `chat_id`.
   - `agent.chat.send` with prompt `"My favorite color is blue. Acknowledge."`.
   - `agent.chat.send` with prompt `"What is my favorite color?"`.
   - `agent.chat.close`.
   - Expect step-3 result to contain `"blue"`. Verifies session-id auto-threading.

8. **`agent.chat.list` shows the open chat with cumulative cost > 0 between turns.**

9. **Closing a non-existent chat returns `{closed: false}` (no error).**

---

## D. Composition

10. **Outer claude uses inner claude's result.**
    - Prompt: "Use `agent.ask` to get 11*13, then double the result, output only the doubled number."
    - Expect: `286`. Verifies the outer claude treats inner-claude output as data.

---

## E. Budget and cost

11. **`agent.budget` reports state with no cap configured.** `{configured: false}` (or `{configured: true, ...}` if cap set in dev TOML).

12. **Hard stop fires.**
    - Set `[budget] max_usd = 0.05` in `.claude-server-dev.toml`.
    - Two `agent.ask` calls in a row.
    - Expect: first succeeds; second returns structured `BudgetExceeded` with `code: "budget_exceeded"`, `total_usd`, `max_usd`.

13. **Reset config back to no budget for normal scenarios.**

---

## F. Streaming

14. **`agent.ask_stream` returns the consolidated result.** Same prompt as scenario 4. Verify final return matches `agent.ask` shape (result text, session_id, cost, num_turns).

15. **Progress notifications observed.** Currently only verifiable from in-server logs (set `RUST_LOG=tower_mcp=debug` in dev). Long-term, want a test harness that captures them.

16. **`agent.chat.send_stream` preserves session continuity.** Repeat scenario 7 but use `chat.send_stream`. Result should be identical.

---

## G. Per-cwd serialization

17. **Two concurrent `agent.ask` calls in the same cwd serialize.**
    - Fire two tool calls in parallel via the outer claude.
    - Expect: total wall time ~= sum of individual times, not max.
    - This is current behavior and intentional.

---

## H. Error mapping

18. **CLI failure surfaces structured.**
    - Configure dev TOML with `[claude] binary = "/nonexistent/claude"`.
    - Any tool call should return `{code: "io", kind: "Io", message: ...}` not a raw error string.

19. **Timeout fires.**
    - Set `[claude] timeout_secs = 1` in dev TOML.
    - Call `claude.doctor` (slow) -> expect `{code: "timeout", kind: "Timeout", timeout_seconds: 1}`.

20. **JSON parse error.**
    - Verify `claude.query_json` returns proper `QueryResult` shape and doesn't bare-integer-parse-fail (regression for the bug fixed in PR #555 dogfooding).

---

## I. Cancellation (when client supports it)

21. **`notifications/cancelled` propagates.**
    - Start a long-running call (e.g. a multi-turn `agent.ask` with a heavy prompt).
    - Send `{"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":<id>}}`.
    - Expect: tool returns early with cancellation error; child process dies promptly.
    - Today's host claude doesn't expose this, so for now: verify by direct JSON-RPC drive.

---

## J. Environmental hygiene (post-isolation work)

Stubbed for when isolation lands. Will validate that spawning claude with isolated `HOME`/`XDG_CONFIG_HOME`/`CLAUDE_CONFIG_DIR` produces a self-contained sandbox dir and doesn't touch the host's real `~/.claude`.

22. **(Future) Spawn writes only to scratch dir.** `find ~/.claude -newer <test-start>` returns nothing.
23. **(Future) Per-chat sandboxes don't cross-contaminate.** Two chats with different sandboxes have distinct session storage.

---

## How to run

For now: pick a scenario, paste the relevant prompt into a `claude -p` invocation with the right `--allowed-tools`, eyeball the result. The whole suite is ~15 minutes if everything works; rather longer if anything's broken.

Eventual: a test binary that drives the server via `tower-mcp::TestClient`, runs each scenario as a #\[test\], gates on response shape. That'd live in `tests/server_scenarios.rs`.
