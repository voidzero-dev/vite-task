use std::{path::Component, sync::Arc};

use vt_path::{AbsolutePath, RelativePathBuf};
use vt_str::Str;

use crate::Error;

pub fn entry_argument(
    entry: &Arc<AbsolutePath>,
    cwd: &AbsolutePath,
    workspace: &AbsolutePath,
) -> Result<Str, Error> {
    let absolute = entry
        .as_path()
        .to_str()
        .ok_or_else(|| Error::NonUtf8ToolEntry { path: Arc::clone(entry) })?;
    // Do not collapse `link/..`: lexical normalization can change the executed file.
    if [entry.as_ref(), cwd, workspace]
        .iter()
        .any(|path| path.as_path().components().any(|component| component == Component::ParentDir))
    {
        return Ok(Str::from(absolute));
    }
    if let (Ok(Some(entry_relative)), Ok(Some(cwd_relative))) =
        (entry.strip_prefix(workspace), cwd.strip_prefix(workspace))
        && let Some(relative) =
            pathdiff::diff_paths(entry_relative.as_path(), cwd_relative.as_path())
        && let Ok(relative) = RelativePathBuf::new(relative)
    {
        // A leading `./` keeps an entry named `-tool.js` from being read as an option.
        if relative.as_str().starts_with("..") {
            return Ok(Str::from(relative.as_str()));
        }
        return Ok(vt_str::format!("./{}", relative.as_str()));
    }
    Ok(Str::from(absolute))
}
