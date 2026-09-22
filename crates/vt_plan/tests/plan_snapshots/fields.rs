use std::collections::BTreeMap;

use monostate::MustBe;
use serde::Deserialize;
use serde_json::Value;
use vt_str::Str;

/// A selection of JSON fields, independent of the plan's types.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Fields {
    All(MustBe!(true)),
    Children(BTreeMap<Str, Self>),
}

impl Fields {
    /// Find outer field names recursively, retaining their ancestors and array positions.
    /// On error, the value may be partially projected and should be discarded.
    pub fn select(&self, value: &mut Value) -> Result<(), Str> {
        match self {
            Self::All(_) => Ok(()),
            Self::Children(children) => {
                if Self::find(value, children)? {
                    Ok(())
                } else {
                    Err("fields did not match any snapshot fields".into())
                }
            }
        }
    }

    fn find(value: &mut Value, fields: &BTreeMap<Str, Self>) -> Result<bool, Str> {
        if fields.is_empty() {
            return Err("fields must be a nonempty table".into());
        }
        Ok(match value {
            Value::Object(object) => {
                for (name, mut value) in std::mem::take(object) {
                    let keep = if let Some(selection) = fields.get(name.as_str()) {
                        selection.select_children(&mut value)?;
                        true
                    } else {
                        Self::find(&mut value, fields)?
                    };
                    if keep {
                        object.insert(name, value);
                    }
                }
                !object.is_empty()
            }
            Value::Array(array) => {
                let mut matched = false;
                for value in array {
                    if Self::find(value, fields)? {
                        matched = true;
                    } else {
                        // Keep positions without confusing an omitted element with a JSON null.
                        *value = Value::String("<unselected>".into());
                    }
                }
                matched
            }
            _ => false,
        })
    }

    /// Once a field matches, nested selections address direct children only.
    fn select_children(&self, value: &mut Value) -> Result<(), Str> {
        let Self::Children(children) = self else {
            return Ok(());
        };
        if children.is_empty() {
            return Err("fields must be a nonempty table".into());
        }
        match value {
            Value::Object(object) => {
                for (name, mut value) in std::mem::take(object) {
                    if let Some(selection) = children.get(name.as_str()) {
                        selection.select_children(&mut value)?;
                        object.insert(name, value);
                    }
                }
            }
            Value::Array(array) => {
                for value in array {
                    self.select_children(value)?;
                }
            }
            // Preserve nulls and scalar type changes instead of making them look absent.
            _ => {}
        }
        Ok(())
    }
}
