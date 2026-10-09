use super::*;
#[test]
fn username_fallback_and_cli_contract() {
    for (user, expected) in [
        (serde_json::json!({"username":"user","name":"name"}), "user"),
        (serde_json::json!({"username":"","name":"name"}), "name"),
        (serde_json::json!({"name":""}), "(unknown)"),
        (serde_json::json!(null), "(unknown)"),
    ] {
        assert_eq!(username(&user), expected);
    }
    assert!(matches!(
        Cli::try_parse_from(["wanderlog-mcp", "serve", "--read-only"])
            .unwrap()
            .command,
        Command::Serve { read_only: true }
    ));
}
