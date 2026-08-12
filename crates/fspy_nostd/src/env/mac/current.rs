use super::thin::{ThinArgs, ThinEnvs, args as thin_args, envs as thin_envs};
use crate::{CStr, Fat, Result, env::Entry};

/// A snapshot of the macOS thin argument and environment iterators.
pub struct Current {
    args: ThinArgs,
    envs: ThinEnvs,
}

impl Current {
    /// Returns a fresh fat C-string iterator over the process arguments.
    #[must_use]
    pub fn args(&self) -> FatArgs {
        FatArgs { inner: self.args.clone() }
    }

    /// Returns a fresh fat C-string iterator over the process environment.
    #[must_use]
    pub fn envs(&self) -> FatEnvs {
        FatEnvs { inner: self.envs.clone() }
    }
}

/// An iterator over process arguments as counted C strings.
pub struct FatArgs {
    inner: ThinArgs,
}

impl Iterator for FatArgs {
    type Item = CStr<'static, Fat>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(CStr::count)
    }
}

/// An iterator over process environment entries as counted C strings.
pub struct FatEnvs {
    inner: ThinEnvs,
}

impl Iterator for FatEnvs {
    type Item = Entry;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|(name, value)| (name, value.map(CStr::count)))
    }
}

/// Snapshots both macOS thin iterators for portable fat iteration.
///
/// [`Current::args`] and [`Current::envs`] have the same item and error types
/// as their Linux counterparts. Construction is currently infallible on
/// macOS; the result preserves that portable signature.
///
/// # Errors
///
/// This function does not currently return an error on macOS.
///
/// # Safety
///
/// Until the snapshot and every view yielded from it are discarded, the
/// caller must ensure that the argument and environment pointer arrays and
/// their strings remain mapped, readable, and immutable and that no new image
/// is executed.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the return type deliberately matches Linux's fallible current() API"
)]
pub unsafe fn current() -> Result<Current> {
    // SAFETY: the caller accepts both thin iterators' validity requirements.
    let args = unsafe { thin_args() };
    // SAFETY: as above for the environment iterator.
    let envs = unsafe { thin_envs() };
    Ok(Current { args, envs })
}

#[cfg(test)]
mod tests {
    use bstr::ByteSlice as _;

    use super::*;

    #[test]
    fn current_has_portable_fat_iterators() {
        // SAFETY: this test does not mutate the argument or environment arrays
        // while their snapshot or borrowed entries are live.
        let current = unsafe { current().unwrap() };

        let argv_zero = current.args().next().unwrap();
        assert_eq!(argv_zero.as_bytes(), std::env::args_os().next().unwrap().as_encoded_bytes());

        let path = current.envs().find(|(name, _)| name.as_bytes() == b"PATH").unwrap().1.unwrap();
        assert_eq!(path.as_bytes(), std::env::var_os("PATH").unwrap().as_encoded_bytes());
    }
}
