#[derive(Clone, Copy, Eq, PartialEq)]
pub struct ForkGeneration(libc::pid_t);

pub struct ForkTracker;

impl ForkTracker {
    #[expect(
        clippy::unnecessary_wraps,
        reason = "keeps construction identical to the fallible Linux fork tracker"
    )]
    #[cfg_attr(
        test,
        expect(dead_code, reason = "the preload constructor is disabled in unit-test builds")
    )]
    pub const fn new() -> nix::Result<Self> {
        Ok(Self)
    }

    #[expect(
        clippy::unused_self,
        reason = "keeps generation lookup identical to the stateful Linux fork tracker"
    )]
    pub fn generation(&self) -> ForkGeneration {
        // SAFETY: getpid has no preconditions and always succeeds.
        ForkGeneration(unsafe { libc::getpid() })
    }
}
