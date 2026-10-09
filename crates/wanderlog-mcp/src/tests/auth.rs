use super::*;
use std::cell::RefCell;
#[derive(Default)]
struct MockStore {
    value: RefCell<Option<String>>,
    fail: bool,
}
impl CredentialStore for MockStore {
    fn load(&self) -> Result<Option<String>> {
        if self.fail {
            bail!("secret-from-native-error")
        }
        Ok(self.value.borrow().clone())
    }
    fn store(&self, v: &str) -> Result<()> {
        *self.value.borrow_mut() = Some(v.into());
        Ok(())
    }
    fn clear(&self) -> Result<bool> {
        Ok(self.value.borrow_mut().take().is_some())
    }
}
#[test]
fn injected_credentials_precedence_lifecycle_and_errors() {
    let store = MockStore::default();
    assert!(load_with(None, &store).is_err());
    store.store("connect.sid=mock-stored; Path=/").unwrap();
    assert_eq!(
        load_with(Some(" "), &store).unwrap(),
        ("mock-stored".into(), Source::Keychain)
    );
    assert_eq!(
        load_with(Some("mock-env"), &store).unwrap(),
        ("mock-env".into(), Source::Env)
    );
    assert!(load_with(Some("bad\ncookie"), &store).is_err());
    assert!(store.clear().unwrap());
    assert!(!store.clear().unwrap());
    let failing = MockStore {
        fail: true,
        ..Default::default()
    };
    assert!(
        !load_with(None, &failing)
            .unwrap_err()
            .to_string()
            .contains("secret-from-native-error")
    );
    assert_eq!(
        load_with(Some("mock-env"), &failing).unwrap().1,
        Source::Env
    );
}
#[cfg(not(target_os = "macos"))]
#[test]
fn native_fallback_is_explicit_and_does_not_persist() {
    assert!(OsCredentialStore.load().unwrap().is_none());
    assert!(!clear().unwrap());
    assert!(store("mock-session").is_err());
    assert!(store("bad\nvalue").is_err());
}
