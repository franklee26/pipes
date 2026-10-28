use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    fs::{self, File},
    io::{self, Read},
    marker::PhantomData,
    ops::Deref,
    path::Path,
};

use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use sha2::{Digest, Sha256};

use crate::{
    CommandResult, CommandSummary, ConsumerApproval, ConsumerSummary, Error, ExecutionSummary,
    ManifestPreview, OutputSummary, Parallelism, Result, StageSummary, validation,
};

mod bounded;
mod checks;
mod promotion;

use checks::Checks;
use promotion::{Promotion, explicit_promotion};

pub(crate) const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

/// Validate a YAML manifest without opening a registry or executing any plan.
///
/// Source paths are relative to the canonical manifest's parent. The source
/// directory must exist, but tools, version files, Git state, and credentials are
/// deliberately not checked. An omitted display name defaults to the app ID.
/// Stage order is preserved, while resolved `after` edges form an acyclic graph.
/// All immediate predecessors are required at joins.
///
/// Check trees allow at most 32 group levels. Parsing is bounded to 1,048,576
/// expanded values, 8 MiB of expanded scalar bytes, and 96 YAML collection levels.
/// These budgets include aliases and are enforced before constructing an
/// unbounded tree. Auto concurrency is preserved, never resolved on this host.
pub fn preview_manifest(path: impl AsRef<Path>) -> Result<ManifestPreview> {
    preview_manifest_checked(path, None)
}

pub(crate) fn preview_manifest_checked(
    path: impl AsRef<Path>,
    expected_hash: Option<&str>,
) -> Result<ManifestPreview> {
    let requested = path.as_ref();
    let path = fs::canonicalize(requested).map_err(|source| Error::ManifestIo {
        path: requested.to_owned(),
        source,
    })?;
    let metadata = fs::metadata(&path).map_err(|source| Error::ManifestIo {
        path: path.clone(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(Error::invalid(&path, "document", "expected a regular file"));
    }
    let file = File::open(&path).map_err(|source| Error::ManifestIo {
        path: path.clone(),
        source,
    })?;
    let metadata = file.metadata().map_err(|source| Error::ManifestIo {
        path: path.clone(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(Error::invalid(&path, "document", "expected a regular file"));
    }
    if metadata.len() > MAX_MANIFEST_BYTES as u64 {
        return Err(too_large(&path));
    }
    // Also bound the read itself: a file may grow after its metadata is read.
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|source| Error::ManifestIo {
            path: path.clone(),
            source,
        })?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(too_large(&path));
    }
    let yaml = String::from_utf8(bytes).map_err(|error| {
        Error::invalid(
            &path,
            "document",
            format!("expected UTF-8 YAML: {}", error.utf8_error()),
        )
    })?;
    if let Some(expected) = expected_hash {
        let actual = format!("{:x}", Sha256::digest(yaml.as_bytes()));
        if expected != actual {
            return Err(Error::ManifestChanged {
                path,
                expected_hash: expected.to_owned(),
                actual_hash: actual,
            });
        }
    }
    let manifest = parse(&path, &yaml)?;
    let parent = path.parent().ok_or_else(|| {
        Error::invalid(
            &path,
            "document",
            "the manifest must have a parent directory",
        )
    })?;
    let source = parent.join(&manifest.source.path.0);
    let source = fs::canonicalize(&source).map_err(|error| source_error(&source, error))?;
    let metadata = fs::metadata(&source).map_err(|error| source_error(&source, error))?;
    if !metadata.is_dir() {
        return Err(Error::invalid(
            &path,
            "source.path",
            format!("'{}' must be a directory", source.display()),
        ));
    }
    manifest.into_preview(&path, &source, yaml)
}

fn source_error(path: &Path, source: io::Error) -> Error {
    if source.kind() == io::ErrorKind::NotFound {
        Error::SourceNotFound {
            path: path.to_owned(),
        }
    } else {
        Error::SourceIo {
            path: path.to_owned(),
            source,
        }
    }
}

fn too_large(path: &Path) -> Error {
    Error::invalid(
        path,
        "document",
        "YAML exceeds the 1 MiB (1048576 byte) limit",
    )
}

fn parse(path: &Path, yaml: &str) -> Result<Manifest> {
    if yaml.len() > MAX_MANIFEST_BYTES {
        return Err(too_large(path));
    }
    let manifest: Manifest = bounded::from_str(yaml).map_err(|error| {
        let field = error.location().map_or_else(
            || "document".to_owned(),
            |location| {
                format!(
                    "document at line {}, column {}",
                    location.line(),
                    location.column()
                )
            },
        );
        Error::invalid(path, field, error.to_string())
    })?;
    manifest.validate(path)?;
    Ok(manifest)
}

#[derive(Clone, Copy)]
pub(crate) enum SnapshotVersion {
    LinearV1,
    GraphV2,
    PlansV3,
    ConsumersV4,
    PromotionsV5,
    RunsV6,
}

/// Read the saved execution opt-in without consulting mutable source files.
pub(crate) fn execution_source_policy(preview: &ManifestPreview) -> Result<bool> {
    let manifest = parse(Path::new(&preview.manifest_path), &preview.manifest_yaml)?;
    if &*manifest.source.snapshot != "working-tree" {
        return Err(Error::execution(
            "git-commit source snapshots are not implemented. To run commands in the current working tree, edit the manifest to set source.snapshot: working-tree, review it, and re-register the app before running",
        ));
    }
    Ok(manifest.source.require_clean)
}

/// Check stored data against its YAML without touching the original files.
pub(crate) fn validate_snapshot(preview: &ManifestPreview, version: SnapshotVersion) -> Result<()> {
    let path = Path::new(&preview.manifest_path);
    let source = Path::new(&preview.source_path);
    for (field, value) in [
        ("manifestPath", preview.manifest_path.as_str()),
        ("sourcePath", preview.source_path.as_str()),
    ] {
        validation::absolute_path(value).map_err(|message| Error::invalid(path, field, message))?;
    }
    let manifest = parse(path, &preview.manifest_yaml)?;
    if matches!(version, SnapshotVersion::LinearV1)
        && let Some(index) = manifest
            .stages
            .iter()
            .position(|stage| stage.after.is_some())
    {
        return Err(Error::invalid(
            path,
            format!("stages[{index}].after"),
            "after was not supported in schema-v1 snapshots",
        ));
    }
    if !matches!(
        version,
        SnapshotVersion::ConsumersV4 | SnapshotVersion::PromotionsV5 | SnapshotVersion::RunsV6
    ) && let Some(index) = manifest
        .stages
        .iter()
        .position(|stage| stage.consumers.is_some())
    {
        return Err(Error::invalid(
            path,
            format!("stages[{index}].consumers"),
            "consumers were not supported in schema-v1/v2/v3 snapshots",
        ));
    }
    if !matches!(
        version,
        SnapshotVersion::PromotionsV5 | SnapshotVersion::RunsV6
    ) && let Some(index) = manifest
        .stages
        .iter()
        .position(|stage| stage.promotion.is_some())
    {
        return Err(Error::invalid(
            path,
            format!("stages[{index}].promotion"),
            "promotion was not supported in schema-v1/v2/v3/v4 snapshots",
        ));
    }
    if !matches!(version, SnapshotVersion::RunsV6) {
        if &*manifest.source.snapshot == "working-tree" {
            return Err(Error::invalid(
                path,
                "source.snapshot",
                "working-tree was not supported in schema-v1/v2/v3/v4/v5 snapshots",
            ));
        }
        for (index, command) in manifest.prepare.iter().enumerate() {
            if command.result.is_some() {
                return Err(unsupported_result(path, &format!("prepare[{index}]")));
            }
        }
        for (index, stage) in manifest.stages.iter().enumerate() {
            stage
                .checks
                .validate_pre_runs(path, &format!("stages[{index}].checks"))?;
        }
    }
    if matches!(
        version,
        SnapshotVersion::LinearV1 | SnapshotVersion::GraphV2
    ) {
        if manifest.execution.max_parallel.is_some() {
            return Err(Error::invalid(
                path,
                "execution.max_parallel",
                "max_parallel was not supported in schema-v1/v2 snapshots",
            ));
        }
        for (index, stage) in manifest.stages.iter().enumerate() {
            stage
                .checks
                .validate_legacy(path, &format!("stages[{index}].checks"))?;
        }
    }
    let expected = manifest.into_preview(path, source, preview.manifest_yaml.clone())?;
    if &expected != preview {
        return Err(Error::invalid(
            path,
            "snapshot",
            "stored preview fields or hash do not match the YAML snapshot",
        ));
    }
    Ok(())
}

fn unsupported_result(path: &Path, field: &str) -> Error {
    Error::invalid(
        path,
        format!("{field}.result"),
        "result was not supported in schema-v1/v2/v3/v4/v5 snapshots",
    )
}

fn path_string(manifest: &Path, field: &str, path: &Path) -> Result<String> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        Error::invalid(
            manifest,
            field,
            format!("path '{}' cannot be represented as UTF-8", path.display()),
        )
    })
}

// deserialize_any prevents YAML numbers and booleans from being coerced to strings.
#[derive(Debug)]
struct Text(String);

impl Deref for Text {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Text {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct TextVisitor;

        impl Visitor<'_> for TextVisitor {
            type Value = Text;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a string")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Text, E> {
                Ok(Text(value.to_owned()))
            }

            fn visit_string<E: de::Error>(self, value: String) -> std::result::Result<Text, E> {
                Ok(Text(value))
            }
        }

        deserializer.deserialize_any(TextVisitor)
    }
}

fn unique_map<'de, D, T>(deserializer: D) -> std::result::Result<BTreeMap<String, T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct UniqueMap<T>(PhantomData<T>);

    impl<'de, T: Deserialize<'de>> Visitor<'de> for UniqueMap<T> {
        type Value = BTreeMap<String, T>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a map with unique string keys")
        }

        fn visit_map<M: MapAccess<'de>>(
            self,
            mut access: M,
        ) -> std::result::Result<Self::Value, M::Error> {
            let mut map = BTreeMap::new();
            while let Some(Text(key)) = access.next_key()? {
                if map.contains_key(&key) {
                    return Err(de::Error::custom(format!("duplicate map key '{key}'")));
                }
                map.insert(key, access.next_value()?);
            }
            Ok(map)
        }
    }

    deserializer.deserialize_map(UniqueMap(PhantomData))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    api: Text,
    app: Text,
    name: Option<Text>,
    source: Source,
    version: Version,
    #[serde(default)]
    execution: Execution,
    #[serde(default)]
    prepare: Vec<Command>,
    #[serde(default, deserialize_with = "unique_map")]
    outputs: BTreeMap<String, Output>,
    stages: Vec<Stage>,
    #[serde(default)]
    retention: Retention,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    path: Text,
    #[serde(default = "git_commit")]
    snapshot: Text,
    #[serde(default = "yes")]
    require_clean: bool,
}

fn git_commit() -> Text {
    Text("git-commit".to_owned())
}

fn yes() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Version {
    #[serde(default = "semver")]
    scheme: Text,
    from: Text,
}

fn semver() -> Text {
    Text("semver".to_owned())
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Execution {
    #[serde(default)]
    host: Host,
    timeout: Option<Text>,
    #[serde(default, deserialize_with = "explicit_parallelism")]
    max_parallel: Option<Parallelism>,
}

fn explicit_parallelism<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Parallelism>, D::Error> {
    Parallelism::deserialize(deserializer).map(Some)
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Host {
    os: Option<Text>,
    arch: Option<Text>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Command {
    id: Text,
    exec: Vec<Text>,
    cwd: Option<Text>,
    timeout: Option<Text>,
    #[serde(default, deserialize_with = "unique_map")]
    env: BTreeMap<String, Text>,
    #[serde(default, deserialize_with = "unique_map")]
    secrets: BTreeMap<String, Text>,
    #[serde(default, deserialize_with = "explicit_result")]
    result: Option<Box<CommandResult>>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case", deny_unknown_fields)]
enum ManifestResult {
    ExitCode {
        success_codes: Vec<i32>,
    },
    JsonFile {
        path: Text,
        pointer: Text,
        success: serde_json::Value,
        failure: serde_json::Value,
    },
}

impl From<ManifestResult> for CommandResult {
    fn from(result: ManifestResult) -> Self {
        match result {
            ManifestResult::ExitCode { success_codes } => Self::ExitCode { success_codes },
            ManifestResult::JsonFile {
                path,
                pointer,
                success,
                failure,
            } => Self::JsonFile {
                path: path.0,
                pointer: pointer.0,
                success,
                failure,
            },
        }
    }
}

fn explicit_result<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Box<CommandResult>>, D::Error> {
    ManifestResult::deserialize(deserializer).map(|result| Some(Box::new(result.into())))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Output {
    path: Text,
    format: Text,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stage {
    id: Text,
    #[serde(default, deserialize_with = "explicit_after")]
    after: Option<Vec<Text>>,
    #[serde(default)]
    checks: Checks,
    #[serde(default)]
    require: Vec<Text>,
    #[serde(default, deserialize_with = "explicit_consumers")]
    consumers: Option<Vec<Consumer>>,
    #[serde(default, deserialize_with = "explicit_promotion")]
    promotion: Option<Promotion>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Consumer {
    id: Text,
    #[serde(default, deserialize_with = "explicit_text")]
    name: Option<Text>,
    target: Text,
    #[serde(default)]
    approval: ConsumerApproval,
}

fn explicit_text<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Text>, D::Error> {
    Text::deserialize(deserializer).map(Some)
}

fn explicit_consumers<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Vec<Consumer>>, D::Error> {
    struct ConsumersVisitor;

    impl<'de> Visitor<'de> for ConsumersVisitor {
        type Value = Option<Vec<Consumer>>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an array of consumer destinations, not null")
        }

        fn visit_seq<S: SeqAccess<'de>>(
            self,
            mut sequence: S,
        ) -> std::result::Result<Self::Value, S::Error> {
            let mut consumers = Vec::new();
            while let Some(consumer) = sequence.next_element()? {
                consumers.push(consumer);
            }
            Ok(Some(consumers))
        }
    }

    deserializer.deserialize_any(ConsumersVisitor)
}

fn explicit_after<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Vec<Text>>, D::Error> {
    struct AfterVisitor;

    impl<'de> Visitor<'de> for AfterVisitor {
        type Value = Option<Vec<Text>>;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("an array of stage ID strings, not null")
        }

        fn visit_seq<S: SeqAccess<'de>>(
            self,
            mut sequence: S,
        ) -> std::result::Result<Self::Value, S::Error> {
            let mut parents = Vec::new();
            while let Some(parent) = sequence.next_element()? {
                parents.push(parent);
            }
            Ok(Some(parents))
        }
    }

    // deserialize_seq would coerce an empty YAML value into an empty sequence.
    deserializer.deserialize_any(AfterVisitor)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Retention {
    #[serde(default = "keep_all")]
    mode: Text,
    #[serde(default = "all_protections")]
    protect: Vec<Text>,
}

fn keep_all() -> Text {
    Text("keep-all".to_owned())
}

fn all_protections() -> Vec<Text> {
    ["stage_heads", "pins", "active_runs"]
        .into_iter()
        .map(|value| Text(value.to_owned()))
        .collect()
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            mode: keep_all(),
            protect: all_protections(),
        }
    }
}

impl Manifest {
    fn validate(&self, path: &Path) -> Result<()> {
        known(path, "api", &self.api, &["pipes/v1"])?;
        slug(path, "app", &self.app)?;
        if let Some(name) = &self.name {
            nonempty(path, "name", name)?;
        }
        nonempty(path, "source.path", &self.source.path)?;
        known(
            path,
            "source.snapshot",
            &self.source.snapshot,
            &["git-commit", "working-tree"],
        )?;
        known(path, "version.scheme", &self.version.scheme, &["semver"])?;
        nonempty(path, "version.from", &self.version.from)?;
        if let Some(os) = &self.execution.host.os {
            known(
                path,
                "execution.host.os",
                os,
                &["macos", "linux", "windows"],
            )?;
        }
        if let Some(arch) = &self.execution.host.arch {
            known(
                path,
                "execution.host.arch",
                arch,
                &["arm64", "aarch64", "x86_64"],
            )?;
        }
        timeout(path, "execution.timeout", self.execution.timeout.as_deref())?;
        commands(path, "prepare", &self.prepare)?;
        let prepare_ids: BTreeSet<&str> = self.prepare.iter().map(|step| &*step.id).collect();
        for (name, output) in &self.outputs {
            let field = format!("outputs.{name}");
            slug(path, &field, name)?;
            nonempty(path, &format!("{field}.path"), &output.path)?;
            known(
                path,
                &format!("{field}.format"),
                &output.format,
                &["file", "directory"],
            )?;
        }
        if self.stages.is_empty() {
            return Err(Error::invalid(
                path,
                "stages",
                "at least one ordered stage is required",
            ));
        }
        let after = self.resolved_after();
        self.validate_graph(path, &after)?;
        for (index, stage) in self.stages.iter().enumerate() {
            let field = format!("stages[{index}]");
            let mut consumer_ids = BTreeSet::new();
            for (index, consumer) in stage.consumers.iter().flatten().enumerate() {
                let field = format!("{field}.consumers[{index}]");
                slug(path, &format!("{field}.id"), &consumer.id)?;
                if !consumer_ids.insert(&consumer.id.0) {
                    return Err(Error::invalid(
                        path,
                        format!("{field}.id"),
                        format!(
                            "duplicate consumer ID '{}' within this stage",
                            consumer.id.0
                        ),
                    ));
                }
                for (key, value, limit) in [
                    (
                        "name",
                        consumer.name.as_deref().unwrap_or(&consumer.id),
                        validation::CONSUMER_NAME_LIMIT,
                    ),
                    (
                        "target",
                        &*consumer.target,
                        validation::CONSUMER_TARGET_LIMIT,
                    ),
                ] {
                    validation::display_text(value, limit).map_err(|message| {
                        Error::invalid(path, format!("{field}.{key}"), message)
                    })?;
                }
            }
            stage.checks.validate(path, &format!("{field}.checks"))?;
            if let Some(promotion) = &stage.promotion {
                promotion.validate(
                    path,
                    &format!("{field}.promotion.criteria"),
                    &stage.checks,
                    &consumer_ids,
                )?;
            }
            for (condition_index, condition) in stage.require.iter().enumerate() {
                let field = format!("{field}.require[{condition_index}]");
                nonempty(path, &field, condition)?;
                match &**condition {
                    "checks.passed" if !stage.checks.has_leaves() => {
                        return Err(Error::invalid(
                            path,
                            field,
                            "checks.passed needs at least one check in this stage",
                        ));
                    }
                    "previous.passed" if after[index].is_empty() => {
                        return Err(Error::invalid(
                            path,
                            field,
                            "previous.passed requires at least one resolved after predecessor; it requires all immediate predecessors to pass",
                        ));
                    }
                    "checks.passed" | "previous.passed" | "approval.local" => {}
                    condition => {
                        let step = condition
                            .strip_prefix("prepare.")
                            .and_then(|value| value.strip_suffix(".passed"));
                        if !step.is_some_and(|id| prepare_ids.contains(id)) {
                            return Err(Error::invalid(
                                path,
                                field,
                                format!(
                                    "unknown condition '{condition}'; expected checks.passed, previous.passed, approval.local, or prepare.<existing-step-id>.passed"
                                ),
                            ));
                        }
                    }
                }
            }
        }
        known(path, "retention.mode", &self.retention.mode, &["keep-all"])?;
        let mut protections = BTreeSet::new();
        for (index, protection) in self.retention.protect.iter().enumerate() {
            let field = format!("retention.protect[{index}]");
            known(
                path,
                &field,
                protection,
                &["stage_heads", "pins", "active_runs"],
            )?;
            if !protections.insert(&protection.0) {
                return Err(Error::invalid(
                    path,
                    field,
                    "duplicate retention protection",
                ));
            }
        }
        Ok(())
    }

    fn resolved_after(&self) -> Vec<Vec<&str>> {
        let mut previous = None;
        self.stages
            .iter()
            .map(|stage| {
                let after = match &stage.after {
                    Some(parents) => parents.iter().map(|parent| &**parent).collect(),
                    None => previous.into_iter().collect(),
                };
                previous = Some(&*stage.id);
                after
            })
            .collect()
    }

    fn validate_graph(&self, path: &Path, after: &[Vec<&str>]) -> Result<()> {
        let mut indices = BTreeMap::new();
        for (index, stage) in self.stages.iter().enumerate() {
            let field = format!("stages[{index}].id");
            slug(path, &field, &stage.id)?;
            if indices.insert(&*stage.id, index).is_some() {
                return Err(Error::invalid(
                    path,
                    field,
                    format!("duplicate stage ID '{}'", stage.id.0),
                ));
            }
        }
        let mut children = vec![Vec::new(); self.stages.len()];
        for (index, parents) in after.iter().enumerate() {
            let mut seen = BTreeSet::new();
            for (parent_index, parent) in parents.iter().enumerate() {
                let field = format!("stages[{index}].after[{parent_index}]");
                slug(path, &field, parent)?;
                if !seen.insert(parent) {
                    return Err(Error::invalid(
                        path,
                        field,
                        format!("duplicate predecessor '{parent}'"),
                    ));
                }
                let parent_stage = indices.get(parent).copied().ok_or_else(|| {
                    Error::invalid(
                        path,
                        &field,
                        format!("unknown predecessor '{parent}'; use an existing stage ID"),
                    )
                })?;
                if parent_stage == index {
                    return Err(Error::invalid(
                        path,
                        field,
                        format!("stage '{parent}' cannot depend on itself"),
                    ));
                }
                children[parent_stage].push(index);
            }
        }
        // Kahn's algorithm validates without recursion or changing declared order.
        let mut remaining: Vec<_> = after.iter().map(Vec::len).collect();
        let mut ready: VecDeque<_> = remaining
            .iter()
            .enumerate()
            .filter_map(|(index, count)| (*count == 0).then_some(index))
            .collect();
        while let Some(parent) = ready.pop_front() {
            for &child in &children[parent] {
                remaining[child] -= 1;
                if remaining[child] == 0 {
                    ready.push_back(child);
                }
            }
        }
        if let Some(index) = remaining.iter().position(|count| *count != 0) {
            return Err(Error::invalid(
                path,
                format!("stages[{index}].after"),
                format!(
                    "dependency cycle detected; stage '{}' is in or depends on a cycle",
                    self.stages[index].id.0
                ),
            ));
        }
        Ok(())
    }

    fn into_preview(self, path: &Path, source: &Path, yaml: String) -> Result<ManifestPreview> {
        let after: Vec<Vec<String>> = self
            .resolved_after()
            .into_iter()
            .map(|parents| parents.into_iter().map(str::to_owned).collect())
            .collect();
        Ok(ManifestPreview {
            app_id: self.app.0.clone(),
            name: self.name.map_or(self.app.0, |name| name.0),
            manifest_path: path_string(path, "manifestPath", path)?,
            source_path: path_string(path, "source.path", source)?,
            manifest_hash: format!("{:x}", Sha256::digest(yaml.as_bytes())),
            manifest_yaml: yaml,
            version_from: self.version.from.0,
            execution: ExecutionSummary {
                os: self.execution.host.os.map(|value| value.0),
                arch: self.execution.host.arch.map(|value| value.0),
                timeout: self.execution.timeout.map(|value| value.0),
                max_parallel: self.execution.max_parallel.unwrap_or(Parallelism::Auto),
            },
            stages: self
                .stages
                .into_iter()
                .zip(after)
                .map(|(stage, after)| {
                    let (checks, check_plan) = stage.checks.into_summaries();
                    StageSummary {
                        requires_approval: stage
                            .require
                            .iter()
                            .any(|condition| &**condition == "approval.local"),
                        id: stage.id.0,
                        after,
                        checks,
                        check_plan,
                        requires: stage.require.into_iter().map(|value| value.0).collect(),
                        promotion_criteria: stage
                            .promotion
                            .map_or_else(Vec::new, |promotion| promotion.criteria),
                        consumers: stage
                            .consumers
                            .unwrap_or_default()
                            .into_iter()
                            .map(|consumer| ConsumerSummary {
                                name: consumer
                                    .name
                                    .map_or_else(|| consumer.id.0.clone(), |name| name.0),
                                id: consumer.id.0,
                                target: consumer.target.0,
                                approval: consumer.approval,
                            })
                            .collect(),
                    }
                })
                .collect(),
            prepare: self.prepare.into_iter().map(CommandSummary::from).collect(),
            outputs: self
                .outputs
                .into_iter()
                .map(|(name, output)| OutputSummary {
                    name,
                    path: output.path.0,
                    format: output.format.0,
                })
                .collect(),
        })
    }
}

impl From<Command> for CommandSummary {
    fn from(command: Command) -> Self {
        Self {
            id: command.id.0,
            exec: command.exec.into_iter().map(|value| value.0).collect(),
            cwd: command.cwd.map(|value| value.0),
            timeout: command.timeout.map(|value| value.0),
            env: command
                .env
                .into_iter()
                .map(|(key, value)| (key, value.0))
                .collect(),
            secrets: command
                .secrets
                .into_iter()
                .map(|(key, value)| (key, value.0))
                .collect(),
            result: command.result.unwrap_or_default(),
        }
    }
}

fn nonempty(path: &Path, field: &str, value: &str) -> Result<()> {
    no_nul(path, field, value)?;
    if value.trim().is_empty() {
        return Err(Error::invalid(path, field, "must be a nonempty string"));
    }
    Ok(())
}

fn no_nul(path: &Path, field: &str, value: &str) -> Result<()> {
    if value.contains('\0') {
        return Err(Error::invalid(
            path,
            field,
            "must not contain NUL characters",
        ));
    }
    Ok(())
}

fn slug(path: &Path, field: &str, value: &str) -> Result<()> {
    validation::slug(value).map_err(|message| Error::invalid(path, field, message))
}

fn known(path: &Path, field: &str, value: &str, allowed: &[&str]) -> Result<()> {
    if !allowed.contains(&value) {
        return Err(Error::invalid(
            path,
            field,
            format!("unknown value '{value}'; expected {}", allowed.join(" or ")),
        ));
    }
    Ok(())
}

fn timeout(path: &Path, field: &str, value: Option<&str>) -> Result<()> {
    if let Some(value) = value {
        nonempty(path, field, value)?;
        let duration = humantime::parse_duration(value).map_err(|error| {
            Error::invalid(path, field, format!("invalid duration '{value}': {error}"))
        })?;
        if duration.is_zero() {
            return Err(Error::invalid(path, field, "duration must be positive"));
        }
    }
    Ok(())
}

fn environment_name(path: &Path, field: &str, name: &str) -> Result<()> {
    let mut bytes = name.bytes();
    let valid = bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if !valid {
        return Err(Error::invalid(
            path,
            field,
            "environment names must match [A-Za-z_][A-Za-z0-9_]*",
        ));
    }
    Ok(())
}

fn command_environment_name(path: &Path, field: &str, name: &str) -> Result<()> {
    environment_name(path, field, name)?;
    if [
        "PIPES_RUN_ID",
        "PIPES_RELEASE_REF",
        "PIPES_STAGE_ID",
        "PIPES_COMMAND_ID",
        "PIPES_RESULT_FILE",
    ]
    .iter()
    .any(|reserved| reserved.eq_ignore_ascii_case(name))
    {
        return Err(Error::invalid(
            path,
            field,
            format!("'{name}' is reserved for the pipeline runner; remove it from env/secrets"),
        ));
    }
    Ok(())
}

fn commands(path: &Path, field: &str, commands: &[Command]) -> Result<()> {
    let mut ids = BTreeSet::new();
    for (index, value) in commands.iter().enumerate() {
        let field = format!("{field}[{index}]");
        slug(path, &format!("{field}.id"), &value.id)?;
        if !ids.insert(&value.id.0) {
            return Err(Error::invalid(
                path,
                format!("{field}.id"),
                format!("duplicate command ID '{}' within this scope", value.id.0),
            ));
        }
        command(path, &field, value)?;
    }
    Ok(())
}

fn command(path: &Path, field: &str, command: &Command) -> Result<()> {
    if command.exec.is_empty() {
        return Err(Error::invalid(
            path,
            format!("{field}.exec"),
            "expected a nonempty array of strings containing an executable",
        ));
    }
    for (index, argument) in command.exec.iter().enumerate() {
        let field = format!("{field}.exec[{index}]");
        if index == 0 {
            nonempty(path, &field, argument)?;
        } else {
            no_nul(path, &field, argument)?;
        }
    }
    if let Some(cwd) = &command.cwd {
        nonempty(path, &format!("{field}.cwd"), cwd)?;
    }
    timeout(
        path,
        &format!("{field}.timeout"),
        command.timeout.as_deref(),
    )?;
    if let Some(result) = &command.result {
        result.validate().map_err(|(key, message)| {
            Error::invalid(path, format!("{field}.result.{key}"), message)
        })?;
    }
    for (name, value) in &command.env {
        let field = format!("{field}.env.{name}");
        command_environment_name(path, &field, name)?;
        no_nul(path, &field, value)?;
    }
    for (name, reference) in &command.secrets {
        let field = format!("{field}.secrets.{name}");
        command_environment_name(path, &field, name)?;
        nonempty(path, &field, reference)?;
        if command.env.contains_key(name) {
            return Err(Error::invalid(
                path,
                field,
                "a name cannot appear in both env and secrets",
            ));
        }
        if let Some(name) = reference.strip_prefix("env://") {
            environment_name(path, &field, name)?;
        } else if let Some(name) = reference.strip_prefix("keychain://") {
            nonempty(path, &field, name)?;
        } else {
            return Err(Error::invalid(
                path,
                field,
                "expected a credential reference env://NAME or keychain://name, not a literal secret",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn saved_source_policy(source: &str) -> ManifestPreview {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("saved-policy.yaml");
        let source_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("missing-policy-source");
        let yaml = format!(
            "api: pipes/v1\napp: demo\nsource: {{{source}}}\nversion: {{from: VERSION}}\nstages: [{{id: build}}]\n"
        );
        parse(&path, &yaml)
            .unwrap()
            .into_preview(&path, &source_path, yaml)
            .unwrap()
    }

    #[test]
    fn execution_policy_reads_only_saved_yaml_and_preserves_the_cleanliness_requirement() {
        for (source, require_clean) in [
            ("path: missing-policy-source, snapshot: working-tree", true),
            (
                "path: missing-policy-source, snapshot: working-tree, require_clean: true",
                true,
            ),
            (
                "path: missing-policy-source, snapshot: working-tree, require_clean: false",
                false,
            ),
        ] {
            let preview = saved_source_policy(source);
            assert_eq!(execution_source_policy(&preview).unwrap(), require_clean);
        }
    }

    #[test]
    fn default_and_explicit_git_commit_policies_require_opt_in_and_reregistration() {
        for source in [
            "path: missing-policy-source",
            "path: missing-policy-source, snapshot: git-commit, require_clean: false",
        ] {
            let preview = saved_source_policy(source);
            let error = execution_source_policy(&preview).unwrap_err();
            assert_eq!(error.code(), "execution");
            for message in [
                "not implemented",
                "source.snapshot: working-tree",
                "re-register",
            ] {
                assert!(error.to_string().contains(message), "{error}");
            }
        }
    }

    #[test]
    fn identifiers_are_ascii_slugs() {
        let path = Path::new("manifest.yaml");
        for value in ["app", "app-2", "2-app"] {
            assert!(slug(path, "app", value).is_ok());
        }
        for value in ["", "-app", "app-", "app--name", "App", "a_b", "été", "a\0"] {
            assert!(slug(path, "app", value).is_err(), "{value:?}");
        }
    }

    #[test]
    fn environment_names_follow_portable_syntax() {
        let path = Path::new("manifest.yaml");
        for value in ["PATH", "_TOKEN2", "lower"] {
            assert!(environment_name(path, "env", value).is_ok());
        }
        for value in ["", "2TOKEN", "APP-NAME", "é", "A=B"] {
            assert!(environment_name(path, "env", value).is_err());
        }
    }
}
