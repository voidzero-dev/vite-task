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

#[async_trait::async_trait]
pub trait SessionHandler<CustomSubcommand>: Send + Sync {
    /// What to spawn for `vite <subcommand_name>`
    async fn process_for_subcommand(
        &mut self,
        subcommand: CustomSubcommand,
    ) -> anyhow::Result<SubcommandProcess>;

    async fn resolve_config(&mut self, package_dir: &Path) -> anyhow::Result<ViteUserConfig>;
}

pub struct Session<CustomSubcommand> {
    handler: Box<dyn SessionHandler<CustomSubcommand>>,
}

pub enum SessionRunArgs {
    CustomSubCommand { subcommand_name: Str, extra_args: Vec<Str> },
}

impl<CustomSubcommand: clap::Subcommand> Session<CustomSubcommand> {
    pub async fn init(
        handler: Box<dyn SessionHandler<CustomSubcommand>>,
    ) -> Result<Self, crate::error::Error> {
        Ok(Self { handler })
    }

    pub async fn run(&mut self, args: CLIArgs<CustomSubcommand>) {}
}
