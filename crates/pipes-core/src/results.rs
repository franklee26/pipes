use std::{
    fmt,
    fs::{self, Metadata, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
    time::SystemTime,
};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{CommandResult, Error, Result};

const MAX_RESULT_BYTES: usize = 1024 * 1024;

#[derive(Debug)]
pub(crate) struct ResultProbe(Probe);

#[derive(Debug)]
enum Probe {
    ExitCode(Vec<i32>),
    JsonFile {
        path: PathBuf,
        pointer: String,
        success: Value,
        failure: Value,
        baseline: Option<Fingerprint>,
    },
}

impl ResultProbe {
    /// Capture freshness before spawning, without deleting or creating result files.
    pub(crate) fn capture(rule: &CommandResult, cwd: &Path) -> Result<Self> {
        rule.validate().map_err(|(field, message)| {
            Error::execution(format!("invalid command result.{field}: {message}"))
        })?;
        Ok(Self(match rule {
            CommandResult::ExitCode { success_codes } => Probe::ExitCode(success_codes.clone()),
            CommandResult::JsonFile {
                path,
                pointer,
                success,
                failure,
            } => {
                let path = cwd.join(path);
                let baseline = read_result(&path)?.map(|(fingerprint, _)| fingerprint);
                Probe::JsonFile {
                    path,
                    pointer: pointer.clone(),
                    success: success.clone(),
                    failure: failure.clone(),
                    baseline,
                }
            }
        }))
    }

    pub(crate) fn path(&self) -> Option<&Path> {
        match &self.0 {
            Probe::ExitCode(_) => None,
            Probe::JsonFile { path, .. } => Some(path),
        }
    }

    pub(crate) fn evaluate(self, exit_code: Option<i32>) -> Result<bool> {
        let Some(exit_code) = exit_code else {
            return Ok(false);
        };
        match self.0 {
            Probe::ExitCode(success_codes) => Ok(success_codes.contains(&exit_code)),
            Probe::JsonFile {
                path,
                pointer,
                success,
                failure,
                baseline,
            } => {
                let (fingerprint, bytes) = read_result(&path)?.ok_or_else(|| {
                    result_error(&path, "was not produced by the command (file is missing)")
                })?;
                if baseline.as_ref() == Some(&fingerprint) {
                    return Err(result_error(
                        &path,
                        "is unchanged from before the command; stale results are not accepted",
                    ));
                }
                let document: Value = serde_json::from_slice(&bytes).map_err(|error| {
                    result_error(&path, format!("contains invalid JSON: {error}"))
                })?;
                let value = document.pointer(&pointer).ok_or_else(|| {
                    result_error(&path, format!("is missing JSON pointer {pointer:?}"))
                })?;
                if value == &success {
                    Ok(true)
                } else if value == &failure {
                    Ok(false)
                } else {
                    Err(result_error(
                        &path,
                        format!(
                            "has an unknown value at JSON pointer {pointer:?}; expected the configured success or failure scalar"
                        ),
                    ))
                }
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Fingerprint {
    stamp: FileStamp,
    digest: [u8; 32],
}

#[derive(Debug, PartialEq, Eq)]
struct FileStamp {
    len: u64,
    modified: Option<SystemTime>,
    created: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed: (i64, i64),
}

impl FileStamp {
    fn new(metadata: &Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;

        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            created: metadata.created().ok(),
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(unix)]
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        }
    }
}

fn result_error(path: &Path, message: impl fmt::Display) -> Error {
    Error::execution(format!("JSON result file '{}': {message}", path.display()))
}

fn regular_file(path: &Path, metadata: &Metadata) -> Result<()> {
    if metadata.file_type().is_symlink() {
        return Err(result_error(path, "must be a regular file, not a symlink"));
    }
    if !metadata.is_file() {
        return Err(result_error(path, "must be a regular file"));
    }
    if metadata.len() > MAX_RESULT_BYTES as u64 {
        return Err(result_error(path, "exceeds the 1 MiB (1048576 byte) limit"));
    }
    Ok(())
}

fn read_result(path: &Path) -> Result<Option<(Fingerprint, Vec<u8>)>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(result_error(
                path,
                format!("cannot inspect the file: {error}"),
            ));
        }
    };
    regular_file(path, &metadata)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Avoid following a replacement symlink or blocking on a replacement FIFO.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = options
        .open(path)
        .map_err(|error| result_error(path, format!("cannot open the file: {error}")))?;
    let before = file
        .metadata()
        .map_err(|error| result_error(path, format!("cannot inspect the opened file: {error}")))?;
    regular_file(path, &before)?;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(MAX_RESULT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| result_error(path, format!("cannot read the file: {error}")))?;
    if bytes.len() > MAX_RESULT_BYTES {
        return Err(result_error(path, "exceeds the 1 MiB (1048576 byte) limit"));
    }
    let after = file
        .metadata()
        .map_err(|error| result_error(path, format!("cannot inspect the read file: {error}")))?;
    regular_file(path, &after)?;
    let current = fs::symlink_metadata(path).map_err(|error| {
        result_error(
            path,
            format!("cannot inspect the result after reading: {error}"),
        )
    })?;
    regular_file(path, &current)?;
    let stamp = FileStamp::new(&after);
    if stamp != FileStamp::new(&before)
        || stamp != FileStamp::new(&current)
        || stamp.len != bytes.len() as u64
    {
        return Err(result_error(
            path,
            "changed while being read; finish writing the complete file before the command exits",
        ));
    }
    Ok(Some((
        Fingerprint {
            stamp,
            digest: Sha256::digest(&bytes).into(),
        },
        bytes,
    )))
}

#[cfg(test)]
mod tests {
    use std::{fs::File, time::Duration};

    use serde_json::json;
    use tempfile::{Builder, TempDir};

    use super::*;

    fn fixture() -> TempDir {
        Builder::new()
            .prefix(".pipes-results-test-")
            .tempdir_in(env!("CARGO_MANIFEST_DIR"))
            .unwrap()
    }

    fn json_rule(path: impl Into<String>) -> CommandResult {
        CommandResult::JsonFile {
            path: path.into(),
            pointer: "/status".to_owned(),
            success: json!("passed"),
            failure: json!("failed"),
        }
    }

    fn error_contains<T: fmt::Debug>(result: Result<T>, expected: &str) {
        let error = result.unwrap_err();
        assert_eq!(error.code(), "execution");
        assert!(error.to_string().contains(expected), "{error}");
    }

    #[test]
    fn exit_codes_use_only_the_configured_statuses_and_need_no_files() {
        for (rule, code, expected) in [
            (CommandResult::default(), Some(0), true),
            (CommandResult::default(), Some(1), false),
            (CommandResult::default(), None, false),
            (
                CommandResult::ExitCode {
                    success_codes: vec![7, 255],
                },
                Some(7),
                true,
            ),
            (
                CommandResult::ExitCode {
                    success_codes: vec![7, 255],
                },
                Some(255),
                true,
            ),
            (
                CommandResult::ExitCode {
                    success_codes: vec![7, 255],
                },
                Some(0),
                false,
            ),
        ] {
            let probe = ResultProbe::capture(&rule, Path::new("missing-command-cwd")).unwrap();
            assert!(probe.path().is_none());
            assert_eq!(probe.evaluate(code).unwrap(), expected);
        }
    }

    #[test]
    fn json_status_overrides_normal_exit_codes_but_never_signal_termination() {
        let root = fixture();
        for (index, (status, code, expected)) in [
            ("passed", Some(0), true),
            ("passed", Some(17), true),
            ("passed", Some(255), true),
            ("failed", Some(0), false),
            ("failed", Some(17), false),
            ("passed", None, false),
        ]
        .into_iter()
        .enumerate()
        {
            let name = format!("result-{index}.json");
            let probe = ResultProbe::capture(&json_rule(&name), root.path()).unwrap();
            fs::write(
                root.path().join(name),
                json!({"status": status}).to_string(),
            )
            .unwrap();
            assert_eq!(probe.evaluate(code).unwrap(), expected);
        }
    }

    #[test]
    fn paths_resolve_against_the_actual_command_cwd_and_preserve_absolute_paths() {
        let root = fixture();
        let cwd = root.path().join("command-cwd");
        fs::create_dir_all(cwd.join("reports")).unwrap();
        let relative = ResultProbe::capture(&json_rule("reports/status.json"), &cwd).unwrap();
        let relative_path = cwd.join("reports/status.json");
        assert_eq!(relative.path(), Some(relative_path.as_path()));
        fs::write(&relative_path, r#"{"status":"passed"}"#).unwrap();
        assert!(relative.evaluate(Some(0)).unwrap());

        let absolute_path = root.path().join("absolute.json");
        let absolute =
            ResultProbe::capture(&json_rule(absolute_path.to_str().unwrap()), &cwd).unwrap();
        assert_eq!(absolute.path(), Some(absolute_path.as_path()));
        fs::write(&absolute_path, r#"{"status":"passed"}"#).unwrap();
        assert!(absolute.evaluate(Some(0)).unwrap());
        assert!(!root.path().join("reports").exists());
    }

    #[test]
    fn scalar_comparisons_preserve_types_and_large_integer_precision() {
        let root = fixture();
        for (index, (success, failure, actual, expected)) in [
            (json!(true), json!(false), json!(true), Some(true)),
            (json!(true), json!(false), json!(false), Some(false)),
            (json!(true), json!(false), json!("true"), None),
            (json!(1), json!(0), json!(true), None),
            (json!(1), json!(0), json!("1"), None),
            (json!(1), json!(0), json!(1), Some(true)),
            (json!(1), json!(0), json!(0), Some(false)),
            (json!(1.5), json!(2.5), json!(1.5), Some(true)),
            (
                json!(9007199254740993_u64),
                json!(9007199254740992_u64),
                json!(9007199254740992_u64),
                Some(false),
            ),
            (json!(""), json!("failed"), json!(""), Some(true)),
            (json!(true), json!("failed"), json!("failed"), Some(false)),
        ]
        .into_iter()
        .enumerate()
        {
            let path = format!("scalar-{index}.json");
            let rule = CommandResult::JsonFile {
                path: path.clone(),
                pointer: "/status".to_owned(),
                success,
                failure,
            };
            let probe = ResultProbe::capture(&rule, root.path()).unwrap();
            fs::write(
                root.path().join(path),
                json!({"status": actual}).to_string(),
            )
            .unwrap();
            match expected {
                Some(expected) => assert_eq!(probe.evaluate(Some(0)).unwrap(), expected),
                None => error_contains(probe.evaluate(Some(0)), "unknown value"),
            }
        }
    }

    #[test]
    fn pointers_support_roots_empty_keys_arrays_unicode_and_rfc6901_escapes() {
        let root = fixture();
        for (index, (pointer, document)) in [
            ("", json!("passed")),
            ("/", json!({"": "passed"})),
            ("/0", json!(["passed"])),
            ("/01", json!({"01": "passed"})),
            (
                "//x~1y/~0key/1",
                json!({"": {"x/y": {"~key": ["failed", "passed"]}}}),
            ),
            ("/~01", json!({"~1": "passed"})),
            ("/a%2Fb", json!({"a%2Fb": "passed"})),
            ("/雪☃", json!({"雪☃": "passed"})),
            ("/\0", json!({"\0": "passed"})),
        ]
        .into_iter()
        .enumerate()
        {
            let path = format!("pointer-{index}.json");
            let rule = CommandResult::JsonFile {
                path: path.clone(),
                pointer: pointer.to_owned(),
                success: json!("passed"),
                failure: json!("failed"),
            };
            let probe = ResultProbe::capture(&rule, root.path()).unwrap();
            fs::write(root.path().join(path), document.to_string()).unwrap();
            assert!(probe.evaluate(Some(0)).unwrap(), "{pointer:?}");
        }
    }

    #[test]
    fn invalid_pointers_and_programmatically_invalid_rules_fail_before_spawn() {
        let root = fixture();
        for pointer in ["status", "#/status", "/~", "/~2", "/~10~", "/~a", "/~雪"] {
            let mut rule = json_rule("status.json");
            if let CommandResult::JsonFile { pointer: value, .. } = &mut rule {
                *value = pointer.to_owned();
            }
            error_contains(ResultProbe::capture(&rule, root.path()), "RFC 6901");
        }
        for codes in [vec![], vec![0, 0], vec![-1], vec![256]] {
            error_contains(
                ResultProbe::capture(
                    &CommandResult::ExitCode {
                        success_codes: codes,
                    },
                    root.path(),
                ),
                "success_codes",
            );
        }
    }

    #[test]
    fn missing_partial_malformed_unknown_and_missing_pointer_results_are_errors() {
        let root = fixture();
        for (index, (bytes, reason)) in [
            (None, "file is missing"),
            (Some(br#"{"status":"#.as_slice()), "invalid JSON"),
            (
                Some(br#"{"status":"passed"} garbage"#.as_slice()),
                "invalid JSON",
            ),
            (Some(b"\xff".as_slice()), "invalid JSON"),
            (Some(br#"{}"#.as_slice()), "missing JSON pointer"),
            (Some(br#"{"status":null}"#.as_slice()), "unknown value"),
            (Some(br#"{"status":{}}"#.as_slice()), "unknown value"),
            (Some(br#"{"status":[]}"#.as_slice()), "unknown value"),
            (Some(br#"{"status":"pending"}"#.as_slice()), "unknown value"),
        ]
        .into_iter()
        .enumerate()
        {
            let path = format!("bad-{index}.json");
            let probe = ResultProbe::capture(&json_rule(&path), root.path()).unwrap();
            if let Some(bytes) = bytes {
                fs::write(root.path().join(path), bytes).unwrap();
            }
            error_contains(probe.evaluate(Some(0)), reason);
        }
    }

    #[test]
    fn array_indices_must_resolve_without_leading_zero_or_append_syntax() {
        let root = fixture();
        for (index, pointer) in ["/01", "/-", "/2"].into_iter().enumerate() {
            let path = format!("array-{index}.json");
            let rule = CommandResult::JsonFile {
                path: path.clone(),
                pointer: pointer.to_owned(),
                success: json!("passed"),
                failure: json!("failed"),
            };
            let probe = ResultProbe::capture(&rule, root.path()).unwrap();
            fs::write(root.path().join(path), r#"["passed", "passed"]"#).unwrap();
            error_contains(probe.evaluate(Some(0)), "missing JSON pointer");
        }
    }

    #[test]
    fn unchanged_preexisting_results_are_stale_and_never_deleted_or_modified() {
        let root = fixture();
        let path = root.path().join("status.json");
        let bytes = br#"{"status":"passed"}"#;
        fs::write(&path, bytes).unwrap();
        let before = fs::metadata(&path).unwrap().modified().unwrap();
        let probe = ResultProbe::capture(&json_rule("status.json"), root.path()).unwrap();
        error_contains(probe.evaluate(Some(0)), "unchanged");
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), before);
    }

    #[test]
    fn a_partial_baseline_may_be_replaced_with_a_complete_fresh_result() {
        let root = fixture();
        let path = root.path().join("status.json");
        fs::write(&path, br#"{"status":"#).unwrap();
        let probe = ResultProbe::capture(&json_rule("status.json"), root.path()).unwrap();
        fs::write(&path, br#"{"status":"passed"}"#).unwrap();
        assert!(probe.evaluate(Some(0)).unwrap());
    }

    #[test]
    fn a_same_length_rewrite_with_preserved_modification_time_is_fresh() {
        let root = fixture();
        let path = root.path().join("status.json");
        fs::write(&path, br#"{"status":"failed"}"#).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let probe = ResultProbe::capture(&json_rule("status.json"), root.path()).unwrap();
        fs::write(&path, br#"{"status":"passed"}"#).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert!(probe.evaluate(Some(0)).unwrap());
    }

    #[test]
    fn identical_contents_with_observably_changed_metadata_are_fresh() {
        let root = fixture();
        let path = root.path().join("status.json");
        fs::write(&path, br#"{"status":"passed"}"#).unwrap();
        let probe = ResultProbe::capture(&json_rule("status.json"), root.path()).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(86400))
            .unwrap();
        assert!(probe.evaluate(Some(0)).unwrap());
    }

    #[test]
    fn oversized_baselines_and_new_files_are_rejected_but_the_exact_limit_is_allowed() {
        let root = fixture();
        let path = root.path().join("status.json");
        let probe = ResultProbe::capture(&json_rule("status.json"), root.path()).unwrap();
        fs::write(&path, vec![b' '; MAX_RESULT_BYTES + 1]).unwrap();
        error_contains(probe.evaluate(Some(0)), "1 MiB");
        error_contains(
            ResultProbe::capture(&json_rule("status.json"), root.path()),
            "1 MiB",
        );

        let probe = ResultProbe::capture(&json_rule("exact.json"), root.path()).unwrap();
        let mut bytes = br#"{"status":"passed"}"#.to_vec();
        bytes.resize(MAX_RESULT_BYTES, b' ');
        fs::write(root.path().join("exact.json"), bytes).unwrap();
        assert!(probe.evaluate(Some(0)).unwrap());
    }

    #[test]
    fn directories_are_rejected_before_and_after_spawning() {
        let root = fixture();
        let path = root.path().join("status.json");
        let probe = ResultProbe::capture(&json_rule("status.json"), root.path()).unwrap();
        fs::create_dir(&path).unwrap();
        error_contains(probe.evaluate(Some(0)), "regular file");
        error_contains(
            ResultProbe::capture(&json_rule("status.json"), root.path()),
            "regular file",
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_including_dangling_links_are_rejected_before_and_after_spawning() {
        use std::os::unix::fs::symlink;

        let root = fixture();
        for (index, target_exists) in [true, false].into_iter().enumerate() {
            let target = root.path().join(format!("target-{index}.json"));
            if target_exists {
                fs::write(&target, br#"{"status":"passed"}"#).unwrap();
            }
            let path = format!("link-{index}.json");
            let probe = ResultProbe::capture(&json_rule(&path), root.path()).unwrap();
            symlink(&target, root.path().join(&path)).unwrap();
            error_contains(probe.evaluate(Some(0)), "symlink");
            error_contains(
                ResultProbe::capture(&json_rule(&path), root.path()),
                "symlink",
            );
            assert_eq!(target.exists(), target_exists);
        }
    }
}
