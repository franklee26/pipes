use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    num::NonZeroU32,
    path::Path,
};

use serde::{
    Deserialize, Deserializer,
    de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor},
};

use crate::{CheckPlanNode, CommandSummary, Error, Parallelism, Result};

use super::{Command, ManifestResult, Text, command, slug, unique_map, unsupported_result};

const MAX_GROUP_DEPTH: usize = 32;

#[derive(Default)]
pub(super) struct Checks(Vec<Node>);

enum Node {
    Command(Command),
    Sequence {
        id: Text,
        steps: Vec<Node>,
    },
    Parallel {
        id: Text,
        steps: Vec<Node>,
        max_parallel: Option<NonZeroU32>,
    },
}

impl<'de> Deserialize<'de> for Checks {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        CheckListSeed { group_depth: 0 }
            .deserialize(deserializer)
            .map(Self)
    }
}

struct CheckListSeed {
    group_depth: usize,
}

impl<'de> DeserializeSeed<'de> for CheckListSeed {
    type Value = Vec<Node>;

    fn deserialize<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Self::Value, D::Error> {
        if self.group_depth == 0 {
            deserializer.deserialize_seq(self)
        } else {
            deserializer.deserialize_any(self)
        }
    }
}

impl<'de> Visitor<'de> for CheckListSeed {
    type Value = Vec<Node>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an ordered list of check nodes (groups must be nonempty and not null)")
    }

    fn visit_seq<A: SeqAccess<'de>>(
        self,
        mut sequence: A,
    ) -> std::result::Result<Self::Value, A::Error> {
        let mut nodes = Vec::new();
        while let Some(node) = sequence.next_element_seed(NodeSeed {
            group_depth: self.group_depth,
        })? {
            nodes.push(node);
        }
        if self.group_depth > 0 && nodes.is_empty() {
            return Err(de::Error::custom(
                "check groups must contain at least one child",
            ));
        }
        Ok(nodes)
    }
}

struct NodeSeed {
    group_depth: usize,
}

impl<'de> DeserializeSeed<'de> for NodeSeed {
    type Value = Node;

    fn deserialize<D: Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<Node, D::Error> {
        deserializer.deserialize_map(self)
    }
}

enum Body {
    Command(Vec<Text>),
    Sequence(Vec<Node>),
    Parallel(Vec<Node>),
}

impl<'de> Visitor<'de> for NodeSeed {
    type Value = Node;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a check node with exactly one of exec, sequence, or parallel")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<Node, A::Error> {
        const FIELDS: &[&str] = &[
            "id",
            "exec",
            "sequence",
            "parallel",
            "max_parallel",
            "cwd",
            "timeout",
            "env",
            "secrets",
            "result",
        ];
        let mut seen = BTreeSet::new();
        let mut id = None;
        let mut body = None;
        let mut cwd = None;
        let mut timeout = None;
        let mut env = BTreeMap::new();
        let mut secrets = BTreeMap::new();
        let mut result = None;
        let mut max_parallel = None;
        while let Some(Text(field)) = map.next_key()? {
            if !seen.insert(field.clone()) {
                return Err(de::Error::custom(format!("duplicate field '{field}'")));
            }
            if matches!(field.as_str(), "exec" | "sequence" | "parallel") && body.is_some() {
                return Err(de::Error::custom(
                    "a check node must have exactly one of exec, sequence, or parallel",
                ));
            }
            match field.as_str() {
                "id" => id = Some(map.next_value()?),
                "exec" => body = Some(Body::Command(map.next_value()?)),
                "sequence" | "parallel" => {
                    if self.group_depth >= MAX_GROUP_DEPTH {
                        return Err(de::Error::custom("check-group nesting exceeds 32 levels"));
                    }
                    let steps = map.next_value_seed(CheckListSeed {
                        group_depth: self.group_depth + 1,
                    })?;
                    body = Some(if field == "sequence" {
                        Body::Sequence(steps)
                    } else {
                        Body::Parallel(steps)
                    });
                }
                "max_parallel" => max_parallel = Some(map.next_value::<GroupLimit>()?.0),
                "cwd" => cwd = map.next_value()?,
                "timeout" => timeout = map.next_value()?,
                "env" => env = map.next_value::<Environment>()?.0,
                "secrets" => secrets = map.next_value::<Environment>()?.0,
                "result" => result = Some(Box::new(map.next_value::<ManifestResult>()?.into())),
                _ => return Err(de::Error::unknown_field(&field, FIELDS)),
            }
        }
        let id = id.ok_or_else(|| de::Error::missing_field("id"))?;
        let body = body.ok_or_else(|| {
            de::Error::custom("a check node requires exactly one of exec, sequence, or parallel")
        })?;
        if !matches!(&body, Body::Command(_)) {
            for field in ["cwd", "timeout", "env", "secrets", "result"] {
                if seen.contains(field) {
                    return Err(de::Error::custom(format!(
                        "{field} is only valid on command nodes, not groups"
                    )));
                }
            }
        }
        if max_parallel.is_some() && !matches!(&body, Body::Parallel(_)) {
            return Err(de::Error::custom(
                "max_parallel is only valid on parallel groups",
            ));
        }
        Ok(match body {
            Body::Command(exec) => Node::Command(Command {
                id,
                exec,
                cwd,
                timeout,
                env,
                secrets,
                result,
            }),
            Body::Sequence(steps) => Node::Sequence { id, steps },
            Body::Parallel(steps) => Node::Parallel {
                id,
                steps,
                max_parallel,
            },
        })
    }
}

struct Environment(BTreeMap<String, Text>);

impl<'de> Deserialize<'de> for Environment {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        unique_map(deserializer).map(Self)
    }
}

struct GroupLimit(NonZeroU32);

impl<'de> Deserialize<'de> for GroupLimit {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        match Parallelism::deserialize(deserializer)? {
            Parallelism::Limit(value) => Ok(Self(value)),
            Parallelism::Auto => Err(de::Error::custom(
                "a parallel group's max_parallel must be a positive integer, not auto",
            )),
        }
    }
}

impl Checks {
    pub(super) fn contains_command(&self, id: &str) -> bool {
        self.0.iter().any(|node| node.contains_command(id))
    }

    pub(super) fn has_leaves(&self) -> bool {
        self.0.iter().any(Node::has_leaves)
    }

    pub(super) fn validate(&self, path: &Path, field: &str) -> Result<()> {
        let mut ids = BTreeSet::new();
        for (index, node) in self.0.iter().enumerate() {
            node.validate(path, &format!("{field}[{index}]"), &mut ids)?;
        }
        Ok(())
    }

    pub(super) fn validate_legacy(&self, path: &Path, field: &str) -> Result<()> {
        for (index, node) in self.0.iter().enumerate() {
            let kind = match node {
                Node::Command(_) => continue,
                Node::Sequence { .. } => "sequence",
                Node::Parallel { .. } => "parallel",
            };
            return Err(Error::invalid(
                path,
                format!("{field}[{index}].{kind}"),
                "check groups were not supported in schema-v1/v2 snapshots",
            ));
        }
        Ok(())
    }

    pub(super) fn validate_pre_runs(&self, path: &Path, field: &str) -> Result<()> {
        for (index, node) in self.0.iter().enumerate() {
            node.validate_pre_runs(path, &format!("{field}[{index}]"))?;
        }
        Ok(())
    }

    pub(super) fn into_summaries(self) -> (Vec<CommandSummary>, Vec<CheckPlanNode>) {
        let mut commands = Vec::new();
        let plan = self
            .0
            .into_iter()
            .map(|node| node.into_summary(&mut commands))
            .collect();
        (commands, plan)
    }
}

impl Node {
    fn validate_pre_runs(&self, path: &Path, field: &str) -> Result<()> {
        match self {
            Self::Command(command) if command.result.is_some() => {
                return Err(unsupported_result(path, field));
            }
            Self::Command(_) => {}
            Self::Sequence { steps, .. } | Self::Parallel { steps, .. } => {
                let kind = if matches!(self, Self::Sequence { .. }) {
                    "sequence"
                } else {
                    "parallel"
                };
                for (index, node) in steps.iter().enumerate() {
                    node.validate_pre_runs(path, &format!("{field}.{kind}[{index}]"))?;
                }
            }
        }
        Ok(())
    }

    fn contains_command(&self, id: &str) -> bool {
        match self {
            Self::Command(command) => &*command.id == id,
            Self::Sequence { steps, .. } | Self::Parallel { steps, .. } => {
                steps.iter().any(|node| node.contains_command(id))
            }
        }
    }

    fn has_leaves(&self) -> bool {
        match self {
            Self::Command(_) => true,
            Self::Sequence { steps, .. } | Self::Parallel { steps, .. } => {
                steps.iter().any(Self::has_leaves)
            }
        }
    }

    fn validate<'a>(&'a self, path: &Path, field: &str, ids: &mut BTreeSet<&'a str>) -> Result<()> {
        let id = match self {
            Self::Command(command) => &command.id,
            Self::Sequence { id, .. } | Self::Parallel { id, .. } => id,
        };
        slug(path, &format!("{field}.id"), id)?;
        if !ids.insert(id) {
            return Err(Error::invalid(
                path,
                format!("{field}.id"),
                format!(
                    "duplicate check node ID '{}' within this stage's check tree",
                    id.0
                ),
            ));
        }
        match self {
            Self::Command(value) => command(path, field, value),
            Self::Sequence { steps, .. } | Self::Parallel { steps, .. } => {
                let kind = if matches!(self, Self::Sequence { .. }) {
                    "sequence"
                } else {
                    "parallel"
                };
                for (index, node) in steps.iter().enumerate() {
                    node.validate(path, &format!("{field}.{kind}[{index}]"), ids)?;
                }
                Ok(())
            }
        }
    }

    fn into_summary(self, commands: &mut Vec<CommandSummary>) -> CheckPlanNode {
        match self {
            Self::Command(value) => {
                let command = CommandSummary::from(value);
                commands.push(command.clone());
                CheckPlanNode::Command { command }
            }
            Self::Sequence { id, steps } => CheckPlanNode::Sequence {
                id: id.0,
                steps: steps
                    .into_iter()
                    .map(|node| node.into_summary(commands))
                    .collect(),
            },
            Self::Parallel {
                id,
                steps,
                max_parallel,
            } => CheckPlanNode::Parallel {
                id: id.0,
                steps: steps
                    .into_iter()
                    .map(|node| node.into_summary(commands))
                    .collect(),
                max_parallel,
            },
        }
    }
}
