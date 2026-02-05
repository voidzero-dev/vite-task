use std::fmt::Debug;

use vite_path::AbsolutePath;

use crate::config::UserRunConfig;

/// Loader trait for loading user configuration files (vite-task.json).
#[async_trait::async_trait(?Send)]
pub trait UserConfigLoader: Debug + Send + Sync {
    async fn load_user_config_file(
        &self,
        package_path: &AbsolutePath,
    ) -> anyhow::Result<Option<UserRunConfig>>;
}

/// A `UserConfigLoader` implementation that only loads `vite-task.json`.
///
/// This is mainly for examples and testing as it does not require Node.js environment.
#[derive(Default, Debug)]
pub struct JsonUserConfigLoader(());

#[async_trait::async_trait(?Send)]
impl UserConfigLoader for JsonUserConfigLoader {
    async fn load_user_config_file(
        &self,
        package_path: &AbsolutePath,
    ) -> anyhow::Result<Option<UserRunConfig>> {
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
        let user_config: UserRunConfig = serde_json::from_value(json_value)?;
        Ok(Some(user_config))
    }
}
