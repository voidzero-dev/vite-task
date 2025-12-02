use std::{
    any,
    collections::HashMap,
    ffi::OsString,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};

use petgraph::prelude::StableDiGraph;
use serde::{Deserialize, Serialize};
use vite_path::AbsolutePath;
use vite_str::Str;

use crate::{ResolvedTask, Workspace, cli::CLIArgs, reporter::Reporter};

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
    workspace: Workspace,
}

/// Parameters of a CLI invocation of Vite Task, including current working directory, CLI args, and envs.
///
/// This may come from a real CLI command, or be parsed from a task script.
pub struct CLIParams<CustomSubcommand: clap::Subcommand> {
    pub cwd: Arc<AbsolutePath>,
    pub args: CLIArgs<CustomSubcommand>,
    pub envs: Arc<HashMap<OsString, OsString>>,
}

impl<CustomSubcommand: clap::Subcommand> Session<CustomSubcommand> {
    pub async fn new(
        cwd: &Arc<AbsolutePath>,
        handler: Box<dyn SessionHandler<CustomSubcommand>>,
    ) -> Result<Self, crate::error::Error> {
        Ok(Self { handler, workspace: Workspace::load(cwd.to_absolute_path_buf(), true)? })
    }

    fn plan(&self, params: CLIParams<CustomSubcommand>) {}

    pub async fn start(
        &mut self,
        params: CLIParams<CustomSubcommand>,
        reporter: Box<dyn Reporter>,
    ) -> anyhow::Result<()> {
        let plan = self.plan(params);
        reporter.report_execution_plan("tree");
        Ok(())
    }
}

///
struct ExecutionPlan {}
