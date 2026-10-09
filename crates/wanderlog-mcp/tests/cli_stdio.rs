use serde_json::{Value, json};
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
fn command() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_wanderlog-mcp"));
    c.env_clear()
        .env("WANDERLOG_COOKIE", "invalid\nsynthetic-cookie")
        .kill_on_drop(true);
    if let Some(profile) = std::env::var_os("LLVM_PROFILE_FILE") {
        c.env("LLVM_PROFILE_FILE", profile);
    }
    c
}
#[tokio::test]
async fn cli_help_errors_and_auth_work_without_credentials_or_terminal() {
    for (args, input, success, expected) in [
        (vec!["--help"], "", true, "serve"),
        (vec!["--version"], "", true, env!("CARGO_PKG_VERSION")),
        (vec!["auth", "login"], "", false, "interactive terminal"),
        (vec!["auth", "set"], "bad\ncookie", false, "cookie"),
        (vec!["auth", "status"], "", true, "Not configured"),
        (vec!["trips"], "", false, "cookie"),
        (vec!["show", "1", "--full"], "", false, "cookie"),
        (vec!["show", "not-a-number"], "", false, "invalid value"),
    ] {
        let mut c = command();
        c.args(args.clone())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .await
            .unwrap();
        let output =
            tokio::time::timeout(std::time::Duration::from_secs(3), child.wait_with_output())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(output.status.success(), success, "{args:?}");
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(combined.contains(expected), "{args:?}: {combined}");
        assert!(!combined.contains("synthetic-cookie"));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let output = command().args(["auth", "clear"]).output().await.unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("No stored session"));
    }
}
async fn send(stdin: &mut tokio::process::ChildStdin, body: Value) {
    stdin.write_all(format!("{}\n",json!({"jsonrpc":"2.0", "id":body["id"],"method":body["method"],"params":body["params"]})).as_bytes()).await.unwrap();
}
#[tokio::test]
async fn stdio_initialization_discovery_and_auth_failure_are_offline() {
    for readonly in [false, true] {
        let mut c = command();
        c.arg("serve");
        if readonly {
            c.arg("--read-only");
        }
        let mut child = c
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        send(&mut stdin,json!({"id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"offline-test","version":"1"}}})).await;
        let first: Value = serde_json::from_str(
            &tokio::time::timeout(std::time::Duration::from_secs(3), lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(first["result"]["serverInfo"]["name"], "wanderlog-mcp");
        assert_eq!(
            first["result"]["serverInfo"]["version"],
            env!("CARGO_PKG_VERSION")
        );
        stdin
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
            .await
            .unwrap();
        send(
            &mut stdin,
            json!({"id":2,"method":"tools/list","params":{}}),
        )
        .await;
        let listed: Value = serde_json::from_str(
            &tokio::time::timeout(std::time::Duration::from_secs(3), lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        let tools = listed["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), if readonly { 5 } else { 7 });
        assert_eq!(tools.iter().any(|v| v["name"] == "apply_edits"), !readonly);
        send(
            &mut stdin,
            json!({"id":3,"method":"tools/call","params":{"name":"list_trips","arguments":{}}}),
        )
        .await;
        let result: Value = serde_json::from_str(
            &tokio::time::timeout(std::time::Duration::from_secs(3), lines.next_line())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(result["result"]["isError"], true);
        assert!(!result.to_string().contains("synthetic-cookie"));
        drop(stdin);
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(3), child.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
    }
}
