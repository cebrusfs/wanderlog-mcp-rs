//! Separate opt-in macOS test with a disposable synthetic account.
#![cfg(target_os = "macos")]
#[test]
#[ignore = "native adapter, may prompt"]
fn isolated_keychain_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let account = dir.path().file_name().unwrap().to_str().unwrap();
    let entry = keyring::Entry::new("wanderlog-mcp-test", account).unwrap();
    struct Cleanup(keyring::Entry);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = self.0.delete_credential();
        }
    }
    let cleanup = Cleanup(entry);
    cleanup.0.set_password("synthetic-session").unwrap();
    assert_eq!(cleanup.0.get_password().unwrap(), "synthetic-session");
    cleanup.0.delete_credential().unwrap();
    assert!(matches!(
        cleanup.0.get_password(),
        Err(keyring::Error::NoEntry)
    ));
}
