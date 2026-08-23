use std::{
    env::{self, join_paths},
    ffi::OsStr,
    iter,
    sync::Arc,
};

use vt::{
    Cli, EnabledCacheConfig, HandledCommand, ScriptCommand, SessionConfig, UserCacheConfig,
    get_path_env, plan_request::SyntheticPlanRequest,
};
use vt_path::AbsolutePath;
use vt_str::Str;

#[derive(Debug, Default)]
pub struct CommandHandler(());

/// Find an executable in `node_modules/.bin` directories up the tree.
///
/// # Errors
///
/// Returns an error if the executable cannot be found in any searched path.
pub fn find_executable(
    path_env: Option<&Arc<OsStr>>,
    cwd: &AbsolutePath,
    executable: &str,
) -> anyhow::Result<Arc<OsStr>> {
    #[expect(
        clippy::disallowed_types,
        reason = "PathBuf required by env::split_paths and which::which_in APIs"
    )]
    let mut paths: Vec<std::path::PathBuf> =
        path_env.map_or_else(Vec::new, |path_env| env::split_paths(path_env).collect());
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

#[expect(
    clippy::allow_attributes,
    reason = "usage-rs derive output does not inherit item-level lint attributes"
)]
#[allow(clippy::disallowed_types, reason = "usage-rs generates parser state with String fields")]
mod tool_args {
    use vt_str::Str;

    /// Arguments that the internal `tool` command forwards to `vtt`.
    #[derive(Debug, usage::Cli)]
    #[usage(bin = "vt tool", unknown_flags = "error", args_override_self = false)]
    pub struct ToolArgs {
        #[usage(double_dash = "automatic", value_name = "ARG")]
        pub args: Vec<Str>,
    }
}

use tool_args::ToolArgs;

#[async_trait::async_trait(?Send)]
impl vt::CommandHandler for CommandHandler {
    async fn handle_command(
        &mut self,
        command: &mut ScriptCommand,
    ) -> anyhow::Result<HandledCommand> {
        match command.program.as_str() {
            "vt" | "vp" => {}
            // `vpr <args>` is shorthand for `vt run <args>`
            "vpr" => {
                command.program = Str::from("vt");
                command.args =
                    iter::once(Str::from("run")).chain(command.args.iter().cloned()).collect();
            }
            _ => return Ok(HandledCommand::Verbatim),
        }
        if command.args.first().is_some_and(|arg| arg == "tool") {
            let argv =
                command.args[1..].iter().map(std::convert::AsRef::as_ref).collect::<Vec<_>>();
            let ToolArgs { args } = ToolArgs::parse_from(&argv)
                .map_err(|error| anyhow::anyhow!(ToolArgs::render_failure(&argv, &error)))?;
            let program = find_executable(get_path_env(&command.envs), &command.cwd, "vtt")?;
            return Ok(HandledCommand::Synthesized(SyntheticPlanRequest {
                program,
                args: args.into_iter().filter(|arg| arg != "--").collect(),
                cache_config: UserCacheConfig::with_config(EnabledCacheConfig {
                    env: None,
                    untracked_env: None,
                    input: None,
                    output: None,
                }),
                envs: Arc::clone(&command.envs),
            }));
        }

        let argv = command.args.iter().map(std::convert::AsRef::as_ref).collect::<Vec<_>>();
        let parsed = Cli::parse_from(&argv)
            .map_err(|error| anyhow::anyhow!(Cli::render_failure(&argv, &error)))?
            .command;
        Ok(HandledCommand::ViteTaskCommand(parsed))
    }
}

/// A `UserConfigLoader` implementation that only loads `vite-task.json`.
///
/// This is mainly for examples and testing as it does not require Node.js environment.
#[derive(Default, Debug)]
pub struct JsonUserConfigLoader(());

#[async_trait::async_trait(?Send)]
impl vt::loader::UserConfigLoader for JsonUserConfigLoader {
    async fn load_user_config_file(
        &self,
        package_path: &AbsolutePath,
    ) -> anyhow::Result<Option<vt::config::UserRunConfig>> {
        let config_path = package_path.join("vite-task.json");
        let config_content = match tokio::fs::read_to_string(&config_path).await {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(err) => return Err(err.into()),
        };
        let json_value: Option<serde_json::Value> = jsonc_parser::parse_to_serde_value(
            &config_content,
            &jsonc_parser::ParseOptions::default(),
        )?;
        let user_config: vt::config::UserRunConfig =
            serde_json::from_value(json_value.unwrap_or_default())?;
        Ok(Some(user_config))
    }
}

#[derive(Default)]
pub struct OwnedSessionConfig {
    command_handler: CommandHandler,
    user_config_loader: JsonUserConfigLoader,
}

impl OwnedSessionConfig {
    pub fn as_config(&mut self) -> SessionConfig<'_> {
        SessionConfig {
            command_handler: &mut self.command_handler,
            user_config_loader: &mut self.user_config_loader,
            program_name: Str::from("vt"),
        }
    }
}
