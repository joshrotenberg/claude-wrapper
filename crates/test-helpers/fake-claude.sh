#!/usr/bin/env bash
# Fake claude binary for tests.
#
# Controlled by environment variables:
#   FAKE_CLAUDE_OUTPUT      - text to return (default: "fake response")
#   FAKE_CLAUDE_EXIT_CODE   - exit code to return (default: 0)
#   FAKE_CLAUDE_SESSION_ID  - session_id in JSON output (default: "fake-session-id")
#   FAKE_CLAUDE_ERROR_MSG   - error message for stderr on non-zero exit
#
# Output format is selected by --output-format argument:
#   stream-json  ->  three NDJSON lines (system/assistant/result)
#   json         ->  single JSON object matching QueryResult
#   (none/other) ->  plain text

OUTPUT="${FAKE_CLAUDE_OUTPUT:-fake response}"
EXIT_CODE="${FAKE_CLAUDE_EXIT_CODE:-0}"
SESSION_ID="${FAKE_CLAUDE_SESSION_ID:-fake-session-id}"
ERROR_MSG="${FAKE_CLAUDE_ERROR_MSG:-command failed}"

if [[ "$EXIT_CODE" -ne 0 ]]; then
    echo "$ERROR_MSG" >&2
    exit "$EXIT_CODE"
fi

# Parse --output-format value from args
OUTPUT_FORMAT=""
args=("$@")
for i in "${!args[@]}"; do
    if [[ "${args[$i]}" == "--output-format" ]]; then
        OUTPUT_FORMAT="${args[$((i+1))]}"
        break
    fi
    if [[ "${args[$i]}" == --output-format=* ]]; then
        OUTPUT_FORMAT="${args[$i]#--output-format=}"
        break
    fi
done

if [[ "$OUTPUT_FORMAT" == "stream-json" ]]; then
    # Emit NDJSON matching real claude's stream-json format.
    printf '{"type":"system","subtype":"init","session_id":"%s","tools":[],"mcp_servers":[]}\n' "$SESSION_ID"
    printf '{"type":"assistant","message":{"id":"msg_fake","type":"message","role":"assistant","content":[{"type":"text","text":"%s"}],"model":"claude-fake","stop_reason":"end_turn","stop_sequence":null,"usage":{"input_tokens":5,"output_tokens":3}},"session_id":"%s"}\n' "$OUTPUT" "$SESSION_ID"
    printf '{"type":"result","subtype":"success","result":"%s","session_id":"%s","cost_usd":0.0,"total_cost_usd":0.0,"num_turns":1,"is_error":false}\n' "$OUTPUT" "$SESSION_ID"
elif [[ "$OUTPUT_FORMAT" == "json" ]]; then
    # Emit a single JSON object matching QueryResult.
    printf '{"result":"%s","session_id":"%s","total_cost_usd":0.0,"num_turns":1,"is_error":false}\n' "$OUTPUT" "$SESSION_ID"
else
    # Plain text.
    printf '%s\n' "$OUTPUT"
fi

exit 0
