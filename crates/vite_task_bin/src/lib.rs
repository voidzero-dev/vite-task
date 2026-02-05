use std::{
    collections::HashMap,
    env::{self, join_paths},
    ffi::OsStr,
    iter,
    path::PathBuf,
    sync::Arc,
};

use clap::Subcommand;
use vite_path::AbsolutePath;
use vite_str::Str;
use vite_task::{
    EnabledCacheConfig, SessionCallbacks, UserCacheConfig, UserTaskOptions, get_path_env,
    plan_request::SyntheticPlanRequest,
};

/// Theses are the custom subcommands that synthesize tasks for vite-task
#[derive(Debug, Subcommand)]
pub enum CustomTaskSubcommand {
    /// oxlint
    #[clap(disable_help_flag = true)]
    Lint {
        #[clap(allow_hyphen_values = true, trailing_var_arg = true)]
        args: Vec<Str>,
    },
    /// vitest
    #[clap(disable_help_flag = true)]
    Test {
        #[clap(allow_hyphen_values = true, trailing_var_arg = true)]
        args: Vec<Str>,
    },
    /// Test command for testing additional_envs feature
    EnvTest {
        /// Environment variable name
        name: Str,
        /// Environment variable value
        value: Str,
    },
}

// These are the subcommands that is not handled by vite-task
#[derive(Debug, Subcommand)]
pub enum NonTaskSubcommand {
    Version,
}

#[derive(Debug, Default)]
pub struct TaskSynthesizer(());

fn find_executable(
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

#[async_trait::async_trait(?Send)]
impl vite_task::TaskSynthesizer<CustomTaskSubcommand> for TaskSynthesizer {
    fn should_synthesize_for_program(&self, program: &str) -> bool {
        program == "vite"
    }

    async fn synthesize_task(
        &mut self,
        subcommand: CustomTaskSubcommand,
        envs: &Arc<HashMap<Arc<OsStr>, Arc<OsStr>>>,
        cwd: &Arc<AbsolutePath>,
    ) -> anyhow::Result<SyntheticPlanRequest> {
        let synthesize_node_modules_bin_task = |subcommand_name: &str,
                                                executable_name: &str,
                                                args: Vec<Str>|
         -> anyhow::Result<SyntheticPlanRequest> {
            let direct_execution_cache_key: Arc<[Str]> =
                iter::once(Str::from(subcommand_name)).chain(args.iter().cloned()).collect();
            Ok(SyntheticPlanRequest {
                program: find_executable(get_path_env(envs), &*cwd, executable_name)?,
                args: args.into(),
                task_options: Default::default(),
                direct_execution_cache_key,
                envs: Arc::clone(envs),
            })
        };

        match subcommand {
            CustomTaskSubcommand::Lint { args } => {
                synthesize_node_modules_bin_task("lint", "oxlint", args)
            }
            CustomTaskSubcommand::Test { args } => {
                synthesize_node_modules_bin_task("test", "vitest", args)
            }
            CustomTaskSubcommand::EnvTest { name, value } => {
                let direct_execution_cache_key: Arc<[Str]> =
                    [Str::from("env-test"), name.clone(), value.clone()].into();

                let mut envs = HashMap::clone(&envs);
                // Update the env var for testing
                envs.insert(
                    Arc::from(OsStr::new(name.as_str())),
                    Arc::from(OsStr::new(value.as_str())),
                );

                Ok(SyntheticPlanRequest {
                    program: find_executable(get_path_env(&envs), &*cwd, "print-env")?,
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
                    direct_execution_cache_key,
                    envs: Arc::new(envs),
                })
            }
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
    task_synthesizer: TaskSynthesizer,
    user_config_loader: JsonUserConfigLoader,
}

impl OwnedSessionCallbacks {
    pub fn as_callbacks(&mut self) -> SessionCallbacks<'_, CustomTaskSubcommand> {
        SessionCallbacks {
            task_synthesizer: &mut self.task_synthesizer,
            user_config_loader: &mut self.user_config_loader,
        }
    }
}
