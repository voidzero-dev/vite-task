use std::{path::Path, sync::Arc};

use clap::Parser;
use vite_path::{AbsolutePath, current_dir};
use vite_str::Str;
use vite_task::{
    cli::CLIArgs as ViteTaskCLIArgs,
    reporter::stream::StreamReporter,
    session::{CLIParams, Session, SessionHandler, SubcommandProcess},
};

#[derive(Parser, Debug, PartialEq, Eq)]
#[clap(disable_help_flag = true)]
enum ViteTaskCustomSubcommand {
    /// oxlint
    Lint {
        #[clap(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<Str>,
    },
    /// vitest
    Test {
        #[clap(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<Str>,
    },
    /// oxfmt
    Fmt {
        #[clap(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<Str>,
    },
}

#[test]
fn test_subcommand() {
    let a = ViteTaskCustomSubcommand::try_parse_from(["vite", "lint", "hello"]);
    assert_eq!(a.unwrap(), ViteTaskCustomSubcommand::Lint { args: vec![Str::from("hello")] });

    let b = ViteTaskCustomSubcommand::try_parse_from(["vite", "lint", "--help"]);
    assert_eq!(b.unwrap(), ViteTaskCustomSubcommand::Lint { args: vec![Str::from("--help")] });
}

#[derive(Parser, Debug)]
enum ViteArgs {
    #[clap(flatten)]
    ViteTaskCLIArgs(ViteTaskCLIArgs<ViteTaskCustomSubcommand>),
}

struct ViteTaskHandler;

#[async_trait::async_trait]
impl SessionHandler<ViteTaskCustomSubcommand> for ViteTaskHandler {
    async fn process_for_subcommand(
        &mut self,
        subcommand: ViteTaskCustomSubcommand,
    ) -> anyhow::Result<SubcommandProcess> {
        let (program, args) = match subcommand {
            ViteTaskCustomSubcommand::Lint { args } => ("oxlint", args),
            ViteTaskCustomSubcommand::Test { args } => ("vitest", args),
            ViteTaskCustomSubcommand::Fmt { args } => ("oxfmt", args),
        };
        Ok(SubcommandProcess { program: program.into(), args })
    }

    async fn resolve_config(
        &mut self,
        package_dir: &std::path::Path,
    ) -> anyhow::Result<vite_task::session::ViteUserConfig> {
        #[derive(serde::Deserialize)]
        struct ViteConfig {
            task: vite_task::session::ViteUserConfig,
        }
        let config_file = tokio::fs::read(package_dir.join("vite.config.json")).await?;
        let vite_config: ViteConfig = serde_json::from_slice(&config_file)?;
        Ok(vite_config.task)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cwd = Arc::<AbsolutePath>::from(current_dir()?);
    let args = ViteArgs::parse();

    let mut session = Session::new(&cwd, Box::new(ViteTaskHandler)).await?;
    match args {
        ViteArgs::ViteTaskCLIArgs(vite_task_args) => {
            session
                .start(
                    CLIParams {
                        cwd,
                        args: vite_task_args,
                        envs: Arc::new(std::env::vars_os().collect()),
                    },
                    Box::new(StreamReporter::default()),
                )
                .await?;
        }
    };
    Ok(())
}
