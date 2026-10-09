use super::normalize;

#[test]
fn normalizes_pasted_forms() {
    assert_eq!(normalize(" s%3Aabc.def ").unwrap(), "s%3Aabc.def");
    assert_eq!(normalize("connect.sid=s%3Aabc.def").unwrap(), "s%3Aabc.def");
    assert_eq!(
        normalize("connect.sid=s%3Aabc; Path=/; HttpOnly").unwrap(),
        "s%3Aabc"
    );
    assert!(normalize("").is_err());
    assert!(normalize("a b").is_err());
}
