use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

use vite_str::Str;

/// Resolved environment variables for a command
#[derive(Debug)]
pub struct ResolvedEnvs {
    /// Environment variables that should be fingerprinted
    /// Use BTreeMap to ensure stable order
    pub fingerprinted_envs: BTreeMap<Str, Arc<str>>,

    /// Environment variables that should be passed through without being fingerprinted
    pub pass_through_envs: HashMap<Str, Arc<str>>,
}
