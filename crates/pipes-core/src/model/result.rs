use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// How an arbitrary command reports success; stdout is never interpreted.
/// Omitted rules accept only exit code zero.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CommandResult {
    ExitCode {
        success_codes: Vec<i32>,
    },
    /// A fresh JSON file of at most 1 MiB, resolved relative to the command's actual cwd.
    /// Normal exit codes are ignored; termination without an exit code fails.
    /// Scalars use typed JSON equality, without string coercion.
    JsonFile {
        path: String,
        pointer: String,
        success: Value,
        failure: Value,
    },
}

impl Default for CommandResult {
    fn default() -> Self {
        Self::ExitCode {
            success_codes: vec![0],
        }
    }
}

impl CommandResult {
    pub(crate) fn is_default(&self) -> bool {
        matches!(self, Self::ExitCode { success_codes } if success_codes == &[0])
    }

    pub(crate) fn validate(&self) -> Result<(), (&'static str, &'static str)> {
        match self {
            Self::ExitCode { success_codes } => {
                if success_codes.is_empty() {
                    return Err(("success_codes", "must contain at least one exit code"));
                }
                let mut seen = BTreeSet::new();
                for code in success_codes {
                    if !(0..=255).contains(code) {
                        return Err(("success_codes", "exit codes must be integers from 0 to 255"));
                    }
                    if !seen.insert(code) {
                        return Err(("success_codes", "exit codes must be unique"));
                    }
                }
            }
            Self::JsonFile {
                path,
                pointer,
                success,
                failure,
            } => {
                if path.trim().is_empty() || path.contains('\0') {
                    return Err(("path", "must be a nonempty string without NUL characters"));
                }
                if !valid_pointer(pointer) {
                    return Err((
                        "pointer",
                        "must be an RFC 6901 JSON pointer: empty or starting with '/', with '~' escaped as '~0' and '/' as '~1'",
                    ));
                }
                for (field, value) in [("success", success), ("failure", failure)] {
                    if !matches!(value, Value::String(_) | Value::Bool(_) | Value::Number(_)) {
                        return Err((
                            field,
                            "must be a non-null JSON scalar (string, boolean, or number)",
                        ));
                    }
                }
                if success == failure {
                    return Err(("failure", "success and failure values must be distinct"));
                }
            }
        }
        Ok(())
    }
}

fn valid_pointer(pointer: &str) -> bool {
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return false;
    }
    let mut bytes = pointer.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'~' && !matches!(bytes.next(), Some(b'0' | b'1')) {
            return false;
        }
    }
    true
}
