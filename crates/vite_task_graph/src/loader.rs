use std::fmt::Debug;

use vite_path::AbsolutePath;

use crate::config::UserConfigFile;

/// Loader trait for loading user configuration files (vite.config.*).
#[async_trait::async_trait(?Send)]
pub trait UserConfigLoader: Debug + Send + Sync {
    async fn load_user_config_file(
        &self,
        package_path: &AbsolutePath,
    ) -> anyhow::Result<UserConfigFile>;
}

/// A `UserConfigLoader` implementation that only loads `vite.config.json`.
///
/// This is mainly for examples and testing as it does not require Node.js environment.
#[derive(Default, Debug)]
pub struct JsonUserConfigLoader(());

#[async_trait::async_trait(?Send)]
impl UserConfigLoader for JsonUserConfigLoader {
    async fn load_user_config_file(
        &self,
        package_path: &AbsolutePath,
    ) -> anyhow::Result<UserConfigFile> {
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
