use std::{
    collections::HashMap,
    env::{self, join_paths},
    ffi::OsStr,
    iter,
    path::PathBuf,
    sync::Arc,
};

use vite_path::AbsolutePath;
use vite_str::Str;
use vite_task::{
    EnabledCacheConfig, SessionCallbacks, UserCacheConfig, UserTaskOptions, get_path_env,
    plan_request::SyntheticPlanRequest,
};

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

fn synthesize_node_modules_bin_task(
    subcommand_name: &str,
    executable_name: &str,
    args: &[Str],
    envs: &Arc<HashMap<Arc<OsStr>, Arc<OsStr>>>,
    cwd: &Arc<AbsolutePath>,
) -> anyhow::Result<SyntheticPlanRequest> {
    let direct_execution_cache_key: Arc<[Str]> =
        iter::once(Str::from(subcommand_name)).chain(args.iter().cloned()).collect();
    Ok(SyntheticPlanRequest {
        program: find_executable(get_path_env(envs), &*cwd, executable_name)?,
        args: args.into(),
        task_options: Default::default(),
        direct_execution_cache_key,
        envs: Arc::clone(envs),
    })
}

#[async_trait::async_trait(?Send)]
impl vite_task::TaskSynthesizer for TaskSynthesizer {
    async fn synthesize_task(
        &mut self,
        program: &str,
        args: &[Str],
        envs: &Arc<HashMap<Arc<OsStr>, Arc<OsStr>>>,
        cwd: &Arc<AbsolutePath>,
    ) -> anyhow::Result<Option<SyntheticPlanRequest>> {
        if program != "vite" {
            return Ok(None);
        }
        let Some(subcommand) = args.first() else {
            return Ok(None);
        };
        let rest = &args[1..];
        match subcommand.as_str() {
            "lint" => {
                Ok(Some(synthesize_node_modules_bin_task("lint", "oxlint", rest, envs, cwd)?))
            }
            "test" => {
                Ok(Some(synthesize_node_modules_bin_task("test", "vitest", rest, envs, cwd)?))
            }
            "env-test" => {
                let name = rest
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("env-test requires a name argument"))?
                    .clone();
                let value = rest
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("env-test requires a value argument"))?
                    .clone();

                let direct_execution_cache_key: Arc<[Str]> =
                    [Str::from("env-test"), name.clone(), value.clone()].into();

                let mut envs = HashMap::clone(&envs);
                envs.insert(
                    Arc::from(OsStr::new(name.as_str())),
                    Arc::from(OsStr::new(value.as_str())),
                );

                Ok(Some(SyntheticPlanRequest {
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
                }))
            }
            _ => Ok(None),
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
    pub fn as_callbacks(&mut self) -> SessionCallbacks<'_> {
        SessionCallbacks {
            task_synthesizer: &mut self.task_synthesizer,
            user_config_loader: &mut self.user_config_loader,
        }
    }
}
