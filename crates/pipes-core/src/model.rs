use std::{collections::BTreeMap, fmt, num::NonZeroU32};

use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{self, Visitor},
};

mod promotion;
pub use promotion::*;
mod result;
pub use result::*;
mod runner;
pub use runner::*;

/// A shared run-wide budget for simultaneous commands, not test-tool threads.
/// The runner resolves `Auto` to host capacity at execution.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Parallelism {
    #[default]
    Auto,
    Limit(NonZeroU32),
}

impl Serialize for Parallelism {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::Auto => serializer.serialize_str("auto"),
            Self::Limit(limit) => serializer.serialize_u32(limit.get()),
        }
    }
}

impl<'de> Deserialize<'de> for Parallelism {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct ParallelismVisitor;

        impl Visitor<'_> for ParallelismVisitor {
            type Value = Parallelism;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("'auto' or a positive integer from 1 to 4294967295")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Self::Value, E> {
                if value == "auto" {
                    Ok(Parallelism::Auto)
                } else {
                    Err(E::invalid_value(de::Unexpected::Str(value), &self))
                }
            }

            fn visit_u64<E: de::Error>(self, value: u64) -> std::result::Result<Self::Value, E> {
                self.visit_u128(u128::from(value))
            }

            fn visit_u128<E: de::Error>(self, value: u128) -> std::result::Result<Self::Value, E> {
                u32::try_from(value)
                    .ok()
                    .and_then(NonZeroU32::new)
                    .map(Parallelism::Limit)
                    .ok_or_else(|| {
                        E::custom("max_parallel must be an integer from 1 to 4294967295")
                    })
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> std::result::Result<Self::Value, E> {
                self.visit_i128(i128::from(value))
            }

            fn visit_i128<E: de::Error>(self, value: i128) -> std::result::Result<Self::Value, E> {
                let value = u128::try_from(value)
                    .map_err(|_| E::custom("max_parallel must be a positive integer"))?;
                self.visit_u128(value)
            }
        }

        deserializer.deserialize_any(ParallelismVisitor)
    }
}

/// Validated configuration and the exact YAML bytes represented as UTF-8.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestPreview {
    pub app_id: String,
    pub name: String,
    pub manifest_path: String,
    pub source_path: String,
    pub manifest_hash: String,
    pub manifest_yaml: String,
    pub version_from: String,
    pub execution: ExecutionSummary,
    pub stages: Vec<StageSummary>,
    pub prepare: Vec<CommandSummary>,
    pub outputs: Vec<OutputSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionSummary {
    pub os: Option<String>,
    pub arch: Option<String>,
    pub timeout: Option<String>,
    pub max_parallel: Parallelism,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StageSummary {
    pub id: String,
    /// Resolved immediate predecessors, in declaration order. All are required.
    /// Omitted YAML `after` inherits the previous listed stage; `[]` is a root.
    pub after: Vec<String>,
    /// All command leaves in depth-first declaration order, for inspection only.
    pub checks: Vec<CommandSummary>,
    /// Sequential roots. Every sequence/parallel child must pass; barriers are retained.
    pub check_plan: Vec<CheckPlanNode>,
    pub requires: Vec<String>,
    pub requires_approval: bool,
    pub consumers: Vec<ConsumerSummary>,
    /// Criteria for approving departure from this stage, not execution gates.
    pub promotion_criteria: Vec<PromotionCriterion>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConsumerApproval {
    #[default]
    Manual,
}

impl<'de> Deserialize<'de> for ConsumerApproval {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct ApprovalVisitor;

        impl Visitor<'_> for ApprovalVisitor {
            type Value = ConsumerApproval;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("the string 'manual'")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<Self::Value, E> {
                match value {
                    "manual" => Ok(ConsumerApproval::Manual),
                    value => Err(E::unknown_variant(value, &["manual"])),
                }
            }
        }

        deserializer.deserialize_any(ApprovalVisitor)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConsumerSummary {
    pub id: String,
    pub name: String,
    pub target: String,
    pub approval: ConsumerApproval,
}

/// An explicit attestation of an external deployment, never an execution request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordConsumerDeployment {
    pub app_id: String,
    pub stage_id: String,
    pub consumer_id: String,
    pub expected_manifest_hash: String,
    pub expected_updated_at: String,
    pub release_ref: String,
    pub notes: Option<String>,
    pub approved: bool,
}

/// Immutable destination and registration provenance captured at approval time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConsumerDeployment {
    /// Opaque decimal text from a positive SQLite insertion ID.
    pub id: String,
    pub app_id: String,
    pub app_name: String,
    pub stage_id: String,
    pub consumer: ConsumerSummary,
    pub release_ref: String,
    pub notes: Option<String>,
    pub manifest_hash: String,
    pub manifest_path: String,
    pub source_path: String,
    pub registration_updated_at: String,
    pub recorded_at: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandSummary {
    pub id: String,
    pub exec: Vec<String>,
    pub cwd: Option<String>,
    pub timeout: Option<String>,
    pub env: BTreeMap<String, String>,
    pub secrets: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "CommandResult::is_default")]
    pub result: Box<CommandResult>,
}

/// A planned check tree, limited to 32 group levels when read from a manifest.
///
/// Parallel caps are additional shared ceilings for all descendants. Effective
/// capacity is the minimum of ancestor/run caps, never their product.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CheckPlanNode {
    Command {
        command: CommandSummary,
    },
    Sequence {
        id: String,
        steps: Vec<CheckPlanNode>,
    },
    Parallel {
        id: String,
        steps: Vec<CheckPlanNode>,
        #[serde(deserialize_with = "required_nullable_limit")]
        max_parallel: Option<NonZeroU32>,
    },
}

fn required_nullable_limit<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<NonZeroU32>, D::Error> {
    Option::<NonZeroU32>::deserialize(deserializer)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputSummary {
    pub name: String,
    pub path: String,
    pub format: String,
}

/// A durable registration snapshot, not a live read of the original manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RegisteredApp {
    pub manifest: ManifestPreview,
    pub registered_at: String,
    pub updated_at: String,
}
