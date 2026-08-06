#!/usr/bin/env bash
# Fake claude binary for tests.
#
# Controlled by environment variables:
#   FAKE_CLAUDE_OUTPUT      - text to return (default: "fake response")
#   FAKE_CLAUDE_EXIT_CODE   - exit code to return (default: 0)
#   FAKE_CLAUDE_SESSION_ID  - session_id in JSON output (default: "fake-session-id")
#   FAKE_CLAUDE_ERROR_MSG   - error message for stderr on non-zero exit
#   FAKE_CLAUDE_DELAY       - seconds to sleep before any output (default: 0)
#   FAKE_CLAUDE_COST_USD    - cost in JSON output (default: 0.0)
#   FAKE_CLAUDE_NUM_TURNS   - num_turns in JSON output (default: 1)
#   FAKE_CLAUDE_FORKED_SESSION_ID - session_id when --fork-session is present
#   FAKE_CLAUDE_OUTPUT_BYTES - if >0, OUTPUT becomes this many 'x' bytes,
#       generated in-process. Use for large-output tests: passing a big
#       string via FAKE_CLAUDE_OUTPUT hits Linux's 128KB per-env-string
#       limit (MAX_ARG_STRLEN) and fails execve with E2BIG.
#   FAKE_CLAUDE_PID_FILE    - if set, write this process's pid there before
#       any delay or output, so tests can observe the process itself
#       (e.g. that dropping an in-flight future kills it).
#
# Output format is selected by --output-format argument:
#   stream-json  ->  three NDJSON lines (system/assistant/result)
#   json         ->  single JSON object matching QueryResult
#   (none/other) ->  plain text

OUTPUT="${FAKE_CLAUDE_OUTPUT:-fake response}"
EXIT_CODE="${FAKE_CLAUDE_EXIT_CODE:-0}"

# Generate large output in-process rather than passing it through the
# environment (which would exceed Linux's per-env-string limit).
if [[ -n "${FAKE_CLAUDE_OUTPUT_BYTES:-}" && "${FAKE_CLAUDE_OUTPUT_BYTES}" -gt 0 ]]; then
    OUTPUT=$(head -c "$FAKE_CLAUDE_OUTPUT_BYTES" /dev/zero | tr '\0' 'x')
fi
SESSION_ID="${FAKE_CLAUDE_SESSION_ID:-fake-session-id}"
ERROR_MSG="${FAKE_CLAUDE_ERROR_MSG:-command failed}"
DELAY="${FAKE_CLAUDE_DELAY:-0}"
COST_USD="${FAKE_CLAUDE_COST_USD:-0.0}"
NUM_TURNS="${FAKE_CLAUDE_NUM_TURNS:-1}"

# Record the pid before any delay so tests can watch the process from
# the moment it starts.
if [[ -n "${FAKE_CLAUDE_PID_FILE:-}" ]]; then
    echo $$ > "$FAKE_CLAUDE_PID_FILE"
fi

# Optional delay before any output - used to test timeouts.
if [[ "$DELAY" -gt 0 ]]; then
    sleep "$DELAY"
fi

if [[ "$EXIT_CODE" -ne 0 ]]; then
    echo "$ERROR_MSG" >&2
    exit "$EXIT_CODE"
fi

# Parse --output-format value and detect --fork-session from args
OUTPUT_FORMAT=""
FORK_SESSION=false
args=("$@")
for i in "${!args[@]}"; do
    if [[ "${args[$i]}" == "--output-format" ]]; then
        OUTPUT_FORMAT="${args[$((i+1))]}"
    fi
    if [[ "${args[$i]}" == --output-format=* ]]; then
        OUTPUT_FORMAT="${args[$i]#--output-format=}"
    fi
    if [[ "${args[$i]}" == "--fork-session" ]]; then
        FORK_SESSION=true
    fi
done

# Override session ID when --fork-session is present.
if [[ "$FORK_SESSION" == "true" && -n "$FAKE_CLAUDE_FORKED_SESSION_ID" ]]; then
    SESSION_ID="$FAKE_CLAUDE_FORKED_SESSION_ID"
fi

if [[ "$OUTPUT_FORMAT" == "stream-json" ]]; then
    # Emit NDJSON matching real claude's stream-json format.
    # Escape the output for safe embedding in a JSON string value.
    ESCAPED_OUTPUT=$(printf '%s' "$OUTPUT" | sed 's/\\/\\\\/g; s/"/\\"/g; s/	/\\t/g')
    printf '{"type":"system","subtype":"init","session_id":"%s","tools":[],"mcp_servers":[]}\n' "$SESSION_ID"
    printf '{"type":"assistant","message":{"id":"msg_fake","type":"message","role":"assistant","content":[{"type":"text","text":"%s"}],"model":"claude-fake","stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":5,"output_tokens":3}},"session_id":"%s"}\n' "$ESCAPED_OUTPUT" "$SESSION_ID"
    printf '{"type":"result","subtype":"success","result":"%s","session_id":"%s","total_cost_usd":%s,"num_turns":%s,"is_error":false}\n' "$ESCAPED_OUTPUT" "$SESSION_ID" "$COST_USD" "$NUM_TURNS"
elif [[ "$OUTPUT_FORMAT" == "json" ]]; then
    # Emit a single JSON object matching QueryResult.
    ESCAPED_OUTPUT=$(printf '%s' "$OUTPUT" | sed 's/\\/\\\\/g; s/"/\\"/g; s/	/\\t/g')
    printf '{"result":"%s","session_id":"%s","total_cost_usd":%s,"num_turns":%s,"is_error":false}\n' "$ESCAPED_OUTPUT" "$SESSION_ID" "$COST_USD" "$NUM_TURNS"
else
    # Plain text.
    printf '%s\n' "$OUTPUT"
fi

exit 0
