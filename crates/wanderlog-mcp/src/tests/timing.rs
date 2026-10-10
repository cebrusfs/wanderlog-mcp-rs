use super::Timing;

#[test]
fn line_lists_stages_in_order_then_total() {
    let mut timing = Timing::start("apply_edits");
    timing.stage("connect");
    timing.stage("submit");
    let line = timing.line();
    assert!(
        line.starts_with("wanderlog-mcp timing: apply_edits connect="),
        "{line}"
    );
    assert!(
        line.contains("ms submit=") && line.contains("ms total="),
        "{line}"
    );
}
