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

type Lazy<T> = LazyLock<T, Box<dyn FnOnce() -> T + Send + Sync>>;

pub struct Session<CustomSubcommand> {
    handler: Box<dyn SessionHandler<CustomSubcommand>>,

    /// Lazily discovered workspace
    lazy_workspace: Arc<Lazy<Result<Workspace, Arc<crate::error::Error>>>>,
    lazy_task_graph: Lazy<StableDiGraph<ResolvedTask, ()>>,
}

/// Parameters of a CLI invocation of Vite Task, including current working directory, CLI args, and envs.
pub struct CLIParams<CustomSubcommand: clap::Subcommand> {
    pub cwd: Arc<AbsolutePath>,
    pub args: CLIArgs<CustomSubcommand>,
    pub envs: HashMap<OsString, OsString>,
}

impl<CustomSubcommand: clap::Subcommand> Session<CustomSubcommand> {
    pub async fn init(
        cwd: &Arc<AbsolutePath>,
        handler: Box<dyn SessionHandler<CustomSubcommand>>,
    ) -> Result<Self, crate::error::Error> {
        Ok(Self { handler })
    }

    pub async fn start(
        &mut self,
        params: CLIParams<CustomSubcommand>,
        reporter: Box<dyn Reporter>,
    ) -> anyhow::Result<()> {
        reporter.report_execution_plan("tree");
        Ok(())
    }
}
