use vite_path::AbsolutePath;

use crate::config::UserConfigFile;

/// Loader trait for loading user configuration files (vite.config.*).
pub trait UserConfigLoader {
    fn load_user_config_file(
        &self,
        package_path: &AbsolutePath,
    ) -> impl std::future::Future<Output = anyhow::Result<UserConfigFile>> + Send;
}

/// A `UserConfigLoader` implementation that only loads `vite.config.json`.
///
/// This is mainly for examples and testing as it does not require Node.js environment.
#[derive(Default, Debug)]
pub struct JsonUserConfigLoader(());

impl UserConfigLoader for JsonUserConfigLoader {
    fn load_user_config_file(
        &self,
        package_path: &AbsolutePath,
    ) -> impl std::future::Future<Output = anyhow::Result<UserConfigFile>> + Send {
        async move {
            let config_path = package_path.join("vite.config.json");
            let config_content = match tokio::fs::read_to_string(&config_path).await {
                Ok(content) => content,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(UserConfigFile { tasks: Default::default() });
                }
                Err(err) => return Err(err.into()),
            };
            let user_config: UserConfigFile = serde_json::from_str(&config_content)?;
            Ok(user_config)
        }
    }
}
