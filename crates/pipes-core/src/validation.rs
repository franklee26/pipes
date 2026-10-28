use std::{
    collections::BTreeSet,
    path::{Component, Path},
};

use chrono::DateTime;

use crate::PromotionCriterion;

pub(crate) const CONSUMER_NAME_LIMIT: usize = 128;
pub(crate) const CONSUMER_TARGET_LIMIT: usize = 1024;

pub(crate) fn slug(value: &str) -> std::result::Result<(), &'static str> {
    if value.split('-').any(|part| {
        part.is_empty()
            || !part
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    }) {
        return Err("expected a lowercase ASCII slug: letters/digits separated by single hyphens");
    }
    Ok(())
}

pub(crate) fn display_text(value: &str, limit: usize) -> std::result::Result<(), String> {
    if value.trim().is_empty() {
        return Err("must be a nonempty string".to_owned());
    }
    if value.chars().any(char::is_control) {
        return Err("must not contain control characters".to_owned());
    }
    if value.chars().count() > limit {
        return Err(format!("must contain at most {limit} Unicode characters"));
    }
    Ok(())
}

pub(crate) fn absolute_path(value: &str) -> std::result::Result<(), &'static str> {
    if value.trim().is_empty() || value.contains('\0') {
        return Err("stored paths must be nonempty and must not contain NUL characters");
    }
    if !Path::new(value).is_absolute()
        || Path::new(value)
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err("stored paths must be normalized absolute paths");
    }
    Ok(())
}

pub(crate) fn manifest_hash(value: &str) -> std::result::Result<(), &'static str> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("expected a 64-character lowercase SHA-256 manifest hash");
    }
    Ok(())
}

pub(crate) fn utc_timestamp(value: &str) -> std::result::Result<(), String> {
    let timestamp = DateTime::parse_from_rfc3339(value)
        .map_err(|error| format!("invalid timestamp: {error}"))?;
    if timestamp.offset().local_minus_utc() != 0 || value.trim() != value {
        return Err("must be a UTC RFC3339 timestamp without surrounding whitespace".to_owned());
    }
    Ok(())
}

pub(crate) fn normalize_ref(value: &str) -> std::result::Result<String, String> {
    if value.chars().any(char::is_control) {
        return Err("must not contain control characters".to_owned());
    }
    let value = value.trim();
    display_text(value, 256)?;
    Ok(value.to_owned())
}

pub(crate) fn normalize_notes(value: Option<&str>) -> std::result::Result<Option<String>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\n' | '\t'))
    {
        return Err("may contain newlines and tabs, but no other control characters".to_owned());
    }
    let value = value.trim();
    if value.chars().count() > 4096 {
        return Err("must contain at most 4096 Unicode characters".to_owned());
    }
    Ok((!value.is_empty()).then(|| value.to_owned()))
}

pub(crate) fn insertion_id(value: &str) -> std::result::Result<(), &'static str> {
    match value.parse::<i64>() {
        Ok(id) if id > 0 && id.to_string() == value => Ok(()),
        _ => Err("expected an opaque positive decimal SQLite insertion ID"),
    }
}

pub(crate) fn harness_counts(total: u32, passed: u32) -> std::result::Result<(), &'static str> {
    if passed > total {
        return Err("passed must not exceed total");
    }
    Ok(())
}

pub(crate) fn promotion_criteria<'a>(
    criteria: impl IntoIterator<Item = &'a PromotionCriterion>,
) -> std::result::Result<(), (String, String)> {
    let mut ids = BTreeSet::new();
    let mut checks = BTreeSet::new();
    let mut consumers = BTreeSet::new();
    for (index, criterion) in criteria.into_iter().enumerate() {
        let invalid =
            |field: &str, message: &str| (format!("[{index}].{field}"), message.to_owned());
        slug(criterion.id()).map_err(|message| invalid("id", message))?;
        if !ids.insert(criterion.id()) {
            return Err(invalid(
                "id",
                "duplicate promotion criterion ID within this stage",
            ));
        }
        match criterion {
            PromotionCriterion::Harness {
                check_id,
                expected_tests,
                minimum_passed,
                ..
            } => {
                slug(check_id).map_err(|message| invalid("check", message))?;
                if !checks.insert(check_id) {
                    return Err(invalid(
                        "check",
                        "duplicate check reference within this stage",
                    ));
                }
                if *minimum_passed == 0 || minimum_passed > expected_tests {
                    return Err(invalid(
                        "minimum_passed",
                        "expected 1 <= minimum_passed <= expected_tests <= 4294967295",
                    ));
                }
            }
            PromotionCriterion::Deployment { consumer_id, .. } => {
                slug(consumer_id).map_err(|message| invalid("consumer", message))?;
                if !consumers.insert(consumer_id) {
                    return Err(invalid(
                        "consumer",
                        "duplicate consumer reference within this stage",
                    ));
                }
            }
        }
    }
    Ok(())
}
