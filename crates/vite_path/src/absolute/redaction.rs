use super::AbsolutePath;

thread_local! {
    pub(crate) static REDACTION_PREFIX: std::cell::RefCell<Option<Box<str>>> = std::cell::RefCell::new(None);
}

#[derive(Debug)]
pub struct RedactionGuard(());

impl Drop for RedactionGuard {
    fn drop(&mut self) {
        REDACTION_PREFIX.set(None);
    }
}

pub fn redact_absolute_paths(prefix: &str) -> RedactionGuard {
    REDACTION_PREFIX.with(|redaction_prefix| {
        let mut redaction_prefix = redaction_prefix.borrow_mut();
        assert!(redaction_prefix.is_none(), "RedactionGuard already active");
        *redaction_prefix = Some(prefix.into());
    });
    RedactionGuard(())
}
