use std::{
    collections::HashMap,
    env::{self, join_paths},
    ffi::OsStr,
    iter,
    path::PathBuf,
    sync::Arc,
};

use clap::Parser;
use vite_path::AbsolutePath;
use vite_str::Str;
use vite_task::{
    Command, EnabledCacheConfig, HandledCommand, ScriptCommand, SessionCallbacks, UserCacheConfig,
    UserTaskOptions, get_path_env, plan_request::SyntheticPlanRequest,
};

#[derive(Debug, Default)]
pub struct CommandHandler(());

pub fn find_executable(
    path_env: Option<&Arc<OsStr>>,
    cwd: &AbsolutePath,
    executable: &str,
) -> anyhow::Result<Arc<OsStr>> {
    let mut paths: Vec<PathBuf> =
        if let Some(path_env) = path_env { env::split_paths(path_env).collect() } else { vec![] };
    let mut current_cwd_parent = cwd;
    loop {
        let node_modules_bin = current_cwd_parent.join("node_modules").join(".bin");
        paths.push(node_modules_bin.as_path().to_path_buf());
        if let Some(parent) = current_cwd_parent.parent() {
            current_cwd_parent = parent;
        } else {
            break;
        }
    }
    let executable_path = which::which_in(executable, Some(join_paths(paths)?), cwd)?;
    Ok(executable_path.into_os_string().into())
}

fn synthesize_node_modules_bin_task(
    executable_name: &str,
    args: &[Str],
    envs: &Arc<HashMap<Arc<OsStr>, Arc<OsStr>>>,
    cwd: &Arc<AbsolutePath>,
) -> anyhow::Result<SyntheticPlanRequest> {
    Ok(SyntheticPlanRequest {
        program: find_executable(get_path_env(envs), &*cwd, executable_name)?,
        args: args.into(),
        task_options: Default::default(),
        envs: Arc::clone(envs),
    })
}

#[derive(Debug, Parser)]
#[command(name = "vite", version)]
pub enum Args {
    Lint {
        #[clap(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<Str>,
    },
    Test {
        #[clap(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<Str>,
    },
    EnvTest {
        name: Str,
        value: Str,
    },
    #[command(flatten)]
    Task(Command),
}

#[async_trait::async_trait(?Send)]
impl vite_task::CommandHandler for CommandHandler {
    async fn handle_command(
        &mut self,
        command: &mut ScriptCommand,
    ) -> anyhow::Result<HandledCommand> {
        match command.program.as_str() {
            "vite" => {}
            // `vpr <args>` is shorthand for `vite run <args>`
            "vpr" => {
                command.program = Str::from("vite");
                command.args =
                    iter::once(Str::from("run")).chain(command.args.iter().cloned()).collect();
            }
            _ => return Ok(HandledCommand::Verbatim),
        }
        let args = Args::try_parse_from(
            std::iter::once(command.program.as_str()).chain(command.args.iter().map(Str::as_str)),
        )?;
        match args {
            Args::Lint { args } => Ok(HandledCommand::Synthesized(
                synthesize_node_modules_bin_task("oxlint", &args, &command.envs, &command.cwd)?,
            )),
            Args::Test { args } => Ok(HandledCommand::Synthesized(
                synthesize_node_modules_bin_task("vitest", &args, &command.envs, &command.cwd)?,
            )),
            Args::EnvTest { name, value } => {
                let mut envs = HashMap::clone(&command.envs);
                envs.insert(
                    Arc::from(OsStr::new(name.as_str())),
                    Arc::from(OsStr::new(value.as_str())),
                );

                Ok(HandledCommand::Synthesized(SyntheticPlanRequest {
                    program: find_executable(get_path_env(&envs), &*command.cwd, "print-env")?,
                    args: [name.clone()].into(),
                    task_options: UserTaskOptions {
                        cache_config: UserCacheConfig::Enabled {
                            cache: None,
                            enabled_cache_config: EnabledCacheConfig {
                                envs: None,
                                pass_through_envs: Some(vec![name]),
                            },
                        },
                        ..Default::default()
                    },
                    envs: Arc::new(envs),
                }))
            }
            Args::Task(cli_command) => Ok(HandledCommand::ViteTaskCommand(cli_command)),
        }
    }
}

/// A `UserConfigLoader` implementation that only loads `vite-task.json`.
///
/// This is mainly for examples and testing as it does not require Node.js environment.
#[derive(Default, Debug)]
pub struct JsonUserConfigLoader(());

#[async_trait::async_trait(?Send)]
impl vite_task::loader::UserConfigLoader for JsonUserConfigLoader {
    async fn load_user_config_file(
        &self,
        package_path: &AbsolutePath,
    ) -> anyhow::Result<Option<vite_task::config::UserRunConfig>> {
        let config_path = package_path.join("vite-task.json");
        let config_content = match tokio::fs::read_to_string(&config_path).await {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(err) => return Err(err.into()),
        };
        let json_value = jsonc_parser::parse_to_serde_value(&config_content, &Default::default())?
            .unwrap_or_default();
        let user_config: vite_task::config::UserRunConfig = serde_json::from_value(json_value)?;
        Ok(Some(user_config))
    }
}

#[derive(Default)]
pub struct OwnedSessionCallbacks {
    command_handler: CommandHandler,
    user_config_loader: JsonUserConfigLoader,
}

impl OwnedSessionCallbacks {
    pub fn as_callbacks(&mut self) -> SessionCallbacks<'_> {
        SessionCallbacks {
            command_handler: &mut self.command_handler,
            user_config_loader: &mut self.user_config_loader,
        }
    }
}
