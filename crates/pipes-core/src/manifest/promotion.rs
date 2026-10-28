use std::{collections::BTreeSet, path::Path};

use serde::{Deserialize, Deserializer};

use super::{Text, checks::Checks};
use crate::{DeploymentRequirement, Error, PromotionCriterion, Result, validation};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Promotion {
    #[serde(deserialize_with = "criteria")]
    pub(super) criteria: Vec<PromotionCriterion>,
}

pub(super) fn explicit_promotion<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Promotion>, D::Error> {
    Promotion::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Criterion {
    Harness {
        id: Text,
        check: Text,
        expected_tests: u32,
        minimum_passed: u32,
    },
    Deployment {
        id: Text,
        consumer: Text,
        requirement: DeploymentRequirement,
    },
}

fn criteria<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<PromotionCriterion>, D::Error> {
    Vec::<Criterion>::deserialize(deserializer).map(|criteria| {
        criteria
            .into_iter()
            .map(|criterion| match criterion {
                Criterion::Harness {
                    id,
                    check,
                    expected_tests,
                    minimum_passed,
                } => PromotionCriterion::Harness {
                    id: id.0,
                    check_id: check.0,
                    expected_tests,
                    minimum_passed,
                },
                Criterion::Deployment {
                    id,
                    consumer,
                    requirement,
                } => PromotionCriterion::Deployment {
                    id: id.0,
                    consumer_id: consumer.0,
                    requirement,
                },
            })
            .collect()
    })
}

impl Promotion {
    pub(super) fn validate(
        &self,
        path: &Path,
        field: &str,
        checks: &Checks,
        consumers: &BTreeSet<&String>,
    ) -> Result<()> {
        if self.criteria.is_empty() {
            return Err(Error::invalid(
                path,
                field,
                "promotion requires a nonempty criteria list",
            ));
        }
        validation::promotion_criteria(&self.criteria)
            .map_err(|(key, message)| Error::invalid(path, format!("{field}{key}"), message))?;
        for (index, criterion) in self.criteria.iter().enumerate() {
            match criterion {
                PromotionCriterion::Harness { check_id, .. }
                    if !checks.contains_command(check_id) =>
                {
                    return Err(Error::invalid(
                        path,
                        format!("{field}[{index}].check"),
                        format!(
                            "'{check_id}' must reference a command leaf in this stage's check plan, not a group, prepare command, or another stage"
                        ),
                    ));
                }
                PromotionCriterion::Deployment { consumer_id, .. }
                    if !consumers.contains(consumer_id) =>
                {
                    return Err(Error::invalid(
                        path,
                        format!("{field}[{index}].consumer"),
                        format!("'{consumer_id}' must reference a consumer declared in this stage"),
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }
}
