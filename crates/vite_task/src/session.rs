use std::path::Path;

use serde::{Deserialize, Serialize};
use vite_str::Str;

use crate::cli::CLIArgs;

// Represents the real subprocess to be spawned for a custom subcommand (vite <subcommand_name> ...)
pub struct SubcommandProcess {
    pub program: Str,
    pub args: Vec<Str>,
}

#[derive(Serialize, Deserialize)]
pub struct ViteUserConfig {}

pub trait SessionHandler<Subcommand>: Send + Sync {
    /// What to spawn for `vite <subcommand_name>`
    fn process_for_subcommand(
        &mut self,
        subcommand: &Subcommand,
    ) -> anyhow::Result<SubcommandProcess>;

    fn resolve_config(&mut self, package_dir: &Path) -> anyhow::Result<ViteUserConfig>;
}

pub struct Session<Subcommand> {
    handler: Box<dyn SessionHandler<Subcommand>>,
}

pub enum SessionRunArgs {
    CustomSubCommand { subcommand_name: Str, extra_args: Vec<Str> },
}

impl<Subcommand: clap::Subcommand> Session<Subcommand> {
    pub async fn init(
        handler: Box<dyn SessionHandler<Subcommand>>,
    ) -> Result<Self, crate::error::Error> {
        Ok(Self { handler })
    }

    pub async fn run(&mut self, args: CLIArgs<Subcommand>) {}
}
