use clap::Parser;
use vite_str::Str;
use vite_task::{
    cli::CLIArgs as ViteTaskCLIArgs,
    session::{Session, SessionHandler, SubcommandProcess},
};

#[derive(Parser, Debug, PartialEq, Eq)]
#[clap(disable_help_flag = true)]
enum ViteTaskSubcommands {
    /// linter
    Lint {
        #[clap(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<Str>,
    },
}

#[test]
fn test_subcommand() {
    let a = ViteTaskSubcommands::try_parse_from(["vite", "lint", "hello"]);
    assert_eq!(a.unwrap(), ViteTaskSubcommands::Lint { args: vec![Str::from("hello")] });

    let b = ViteTaskSubcommands::try_parse_from(["vite", "lint", "--help"]);
    assert_eq!(b.unwrap(), ViteTaskSubcommands::Lint { args: vec![Str::from("--help")] });
}

#[derive(Parser, Debug)]
enum ViteArgs {
    Dev,
    #[clap(flatten)]
    ViteTaskCLIArgs(ViteTaskCLIArgs<ViteTaskSubcommands>),
}

struct ViteTaskHandler;

#[async_trait::async_trait]
impl SessionHandler<ViteTaskSubcommands> for ViteTaskHandler {
    fn process_for_subcommand(
        &mut self,
        subcommand: &ViteTaskSubcommands,
    ) -> anyhow::Result<SubcommandProcess> {
        match subcommand {
            ViteTaskSubcommands::Lint { args } => {
                Ok(SubcommandProcess { program: Str::from("oxlint"), args: args.clone() })
            }
        }
    }

    async fn resolve_config(
        &mut self,
        package_dir: &std::path::Path,
    ) -> anyhow::Result<vite_task::session::ViteUserConfig> {
        struct ViteConfig {
            task: vite_task::session::ViteUserConfig,
        }
        todo!()
    }
}

#[tokio::main]
async fn main() {
    let args = ViteArgs::parse();

    let mut session = Session::init(Box::new(ViteTaskHandler)).await.unwrap();
    match dbg!(args) {
        ViteArgs::Dev => {
            println!("vite dev mode");
        }
        ViteArgs::ViteTaskCLIArgs(vite_task_args) => {
            session.run(vite_task_args).await;
        }
    }
}
