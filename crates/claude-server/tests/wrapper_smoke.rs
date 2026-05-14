//! Live wrapper smoke tests -- isolate wrapper-vs-server issues.
//! Run with `cargo test -p claude-server --test wrapper_smoke -- --ignored`.

use claude_wrapper::{Claude, ClaudeCommand, QueryCommand};

#[tokio::test]
#[ignore = "spawns real claude binary"]
async fn wrapper_execute_returns_text_stdout() {
    let claude = Claude::builder().build().expect("claude");
    let q = QueryCommand::new("Reply with exactly OK").model("haiku");
    let out = q.execute(&claude).await.expect("execute");
    eprintln!("raw stdout: <<<{}>>>", out.stdout);
    eprintln!("raw stderr: <<<{}>>>", out.stderr);
    eprintln!("exit: {}", out.exit_code);
    assert!(out.success, "non-zero exit: {}", out.exit_code);
}

#[tokio::test]
#[ignore = "spawns real claude binary"]
async fn wrapper_execute_json_parses() {
    let claude = Claude::builder().build().expect("claude");
    let q = QueryCommand::new("Reply with exactly OK").model("haiku");
    let result = q.execute_json(&claude).await;
    eprintln!("execute_json: {result:?}");
    let parsed = result.expect("execute_json");
    eprintln!("result: {:?}", parsed.result);
    eprintln!("session_id: {:?}", parsed.session_id);
    eprintln!("cost_usd: {:?}", parsed.cost_usd);
    assert!(!parsed.session_id.is_empty(), "missing session id");
}

#[tokio::test]
#[ignore = "spawns real claude binary"]
async fn wrapper_execute_with_output_format_arg() {
    // Drive execute() (not execute_json) with the exact arg set
    // execute_json would build, so we can capture stdout verbatim.
    use claude_wrapper::OutputFormat;
    let claude = Claude::builder().build().expect("claude");
    let q = QueryCommand::new("Reply with exactly OK")
        .model("haiku")
        .output_format(OutputFormat::Json);
    let out = q.execute(&claude).await.expect("execute");
    eprintln!("stdout: <<<{}>>>", out.stdout);
    eprintln!("stderr: <<<{}>>>", out.stderr);
    eprintln!("exit: {}", out.exit_code);
    assert!(out.success);
}
