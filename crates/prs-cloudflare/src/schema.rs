//! Read-only schema readiness for the deployed Worker release.
//!
//! Wrangler records successful D1 migrations in `d1_migrations`. The Worker
//! can use that history through its DB binding, but it must never apply a
//! migration from a request. The release requirement below is part of the
//! Worker binary, so an old Worker cannot report readiness for a newer
//! release merely because the database is healthy for the old code.

use serde::{Deserialize, Serialize};
use worker::D1Database;

/// The schema level required by this Worker release.
///
/// Update this declaration in the same change as a schema-dependent release.
/// A migration must be applied and verified before that release is deployed.
pub const RELEASE_SCHEMA_REQUIREMENT: SchemaRequirement = SchemaRequirement {
    migration_id: 7,
    migration_name: "0007_remove_owner_identity.sql",
};

const KNOWN_MIGRATIONS: &[&str] = &[
    "0001_initial.sql",
    "0002_metadata_schema_upgrade.sql",
    "0003_authorization_credential_name.sql",
    "0004_approved_authorization_expiry.sql",
    "0005_bundle_cleanup_lifecycle.sql",
    "0006_authorization_maintenance.sql",
    "0007_remove_owner_identity.sql",
];

/// The migration identity that a release requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct SchemaRequirement {
    pub migration_id: u32,
    pub migration_name: &'static str,
}

/// One successful migration recorded by Wrangler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppliedMigration {
    pub migration_id: u32,
    pub migration_name: String,
}

/// A controlled reason for a 503 readiness response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadinessFailure {
    MissingHistory,
    HistoryIncomplete,
    MigrationNameMismatch,
    SchemaOutdated,
    HistoryUnavailable,
}

impl ReadinessFailure {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::MissingHistory => "migration_history_missing",
            Self::HistoryIncomplete => "migration_history_incomplete",
            Self::MigrationNameMismatch => "migration_name_mismatch",
            Self::SchemaOutdated => "schema_outdated",
            Self::HistoryUnavailable => "migration_history_unavailable",
        }
    }
}

/// The result returned by the read-only readiness check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaReadiness {
    pub(crate) requirement: SchemaRequirement,
    pub(crate) applied_migration: Option<AppliedMigration>,
    pub(crate) failure: Option<ReadinessFailure>,
}

impl SchemaReadiness {
    pub(crate) fn ready(
        requirement: SchemaRequirement,
        applied_migration: AppliedMigration,
    ) -> Self {
        Self {
            requirement,
            applied_migration: Some(applied_migration),
            failure: None,
        }
    }

    pub(crate) fn not_ready(
        requirement: SchemaRequirement,
        applied_migration: Option<AppliedMigration>,
        failure: ReadinessFailure,
    ) -> Self {
        Self {
            requirement,
            applied_migration,
            failure: Some(failure),
        }
    }

    pub(crate) fn unavailable() -> Self {
        Self::not_ready(
            RELEASE_SCHEMA_REQUIREMENT,
            None,
            ReadinessFailure::HistoryUnavailable,
        )
    }

    pub(crate) const fn is_ready(&self) -> bool {
        self.failure.is_none()
    }
}

#[derive(Debug, Deserialize)]
struct D1MigrationRow {
    id: i32,
    name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MigrationRecord {
    pub(crate) migration_id: u32,
    pub(crate) migration_name: String,
}

/// Read migration history through the Worker D1 binding. This query is
/// intentionally read-only; it does not call the D1 management API or run
/// SQL migrations.
pub(crate) async fn inspect(database: D1Database) -> worker::Result<SchemaReadiness> {
    let rows = database
        .prepare("SELECT id, name FROM d1_migrations ORDER BY id ASC")
        .all()
        .await?
        .results::<D1MigrationRow>()?;
    let mut history = Vec::with_capacity(rows.len());
    for row in rows {
        let migration_id = match u32::try_from(row.id) {
            Ok(migration_id) => migration_id,
            Err(_) => {
                return Ok(SchemaReadiness::not_ready(
                    RELEASE_SCHEMA_REQUIREMENT,
                    None,
                    ReadinessFailure::HistoryIncomplete,
                ))
            }
        };
        history.push(MigrationRecord {
            migration_id,
            migration_name: row.name,
        });
    }
    Ok(assess_migration_history(
        &history,
        RELEASE_SCHEMA_REQUIREMENT,
    ))
}

/// Assess a Wrangler history for a release requirement.
///
/// The required prefix must be contiguous and must retain the exact
/// migration names that the release was built against. Later contiguous
/// migrations are allowed because additive migrations preserve old Worker
/// compatibility during a staged rollout. The later release still has to
/// check its own higher requirement after deployment.
pub(crate) fn assess_migration_history(
    history: &[MigrationRecord],
    requirement: SchemaRequirement,
) -> SchemaReadiness {
    let applied_migration = history.last().map(|migration| AppliedMigration {
        migration_id: migration.migration_id,
        migration_name: migration.migration_name.clone(),
    });

    if history.is_empty() {
        return SchemaReadiness::not_ready(requirement, None, ReadinessFailure::MissingHistory);
    }

    for (index, migration) in history.iter().enumerate() {
        let expected_id = u32::try_from(index + 1).unwrap_or(u32::MAX);
        if migration.migration_id != expected_id || migration.migration_name.is_empty() {
            return SchemaReadiness::not_ready(
                requirement,
                applied_migration,
                ReadinessFailure::HistoryIncomplete,
            );
        }

        if let Some(expected_name) = KNOWN_MIGRATIONS.get(index) {
            if migration.migration_name != *expected_name {
                return SchemaReadiness::not_ready(
                    requirement,
                    applied_migration,
                    ReadinessFailure::MigrationNameMismatch,
                );
            }
        } else if !future_migration_name_matches_id(migration) {
            return SchemaReadiness::not_ready(
                requirement,
                applied_migration,
                ReadinessFailure::MigrationNameMismatch,
            );
        }
    }

    let Some(required_migration) = history
        .iter()
        .find(|migration| migration.migration_id == requirement.migration_id)
    else {
        return SchemaReadiness::not_ready(
            requirement,
            applied_migration,
            ReadinessFailure::SchemaOutdated,
        );
    };

    if required_migration.migration_name != requirement.migration_name {
        return SchemaReadiness::not_ready(
            requirement,
            applied_migration,
            ReadinessFailure::MigrationNameMismatch,
        );
    }

    SchemaReadiness::ready(requirement, applied_migration.expect("non-empty history"))
}

fn future_migration_name_matches_id(migration: &MigrationRecord) -> bool {
    let prefix = format!("{:04}_", migration.migration_id);
    migration.migration_name.starts_with(&prefix)
        && migration.migration_name.ends_with(".sql")
        && migration.migration_name.len() > prefix.len() + ".sql".len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(names: &[&str]) -> Vec<MigrationRecord> {
        names
            .iter()
            .enumerate()
            .map(|(index, name)| MigrationRecord {
                migration_id: (index + 1) as u32,
                migration_name: (*name).to_owned(),
            })
            .collect()
    }

    fn current_history() -> Vec<MigrationRecord> {
        history(KNOWN_MIGRATIONS)
    }

    #[test]
    fn current_release_is_ready_for_its_complete_history() {
        let result = assess_migration_history(&current_history(), RELEASE_SCHEMA_REQUIREMENT);
        assert!(result.is_ready());
        assert_eq!(result.applied_migration.unwrap().migration_id, 7);
    }

    #[test]
    fn empty_history_is_not_ready() {
        let result = assess_migration_history(&[], RELEASE_SCHEMA_REQUIREMENT);
        assert_eq!(result.failure, Some(ReadinessFailure::MissingHistory));
    }

    #[test]
    fn old_history_is_not_ready_for_a_new_release() {
        let new_requirement = SchemaRequirement {
            migration_id: 8,
            migration_name: "0008_additive_column.sql",
        };
        let result = assess_migration_history(&current_history(), new_requirement);
        assert_eq!(result.failure, Some(ReadinessFailure::SchemaOutdated));
    }

    #[test]
    fn an_old_release_accepts_a_contiguous_additive_schema() {
        let mut upgraded_history = current_history();
        upgraded_history.push(MigrationRecord {
            migration_id: 8,
            migration_name: "0008_additive_column.sql".to_owned(),
        });
        let result = assess_migration_history(&upgraded_history, RELEASE_SCHEMA_REQUIREMENT);
        assert!(result.is_ready());
        assert_eq!(result.applied_migration.unwrap().migration_id, 8);
    }

    #[test]
    fn incomplete_history_is_not_ready() {
        let mut incomplete = current_history();
        incomplete.push(MigrationRecord {
            migration_id: 9,
            migration_name: "0009_later.sql".to_owned(),
        });
        let result = assess_migration_history(&incomplete, RELEASE_SCHEMA_REQUIREMENT);
        assert_eq!(result.failure, Some(ReadinessFailure::HistoryIncomplete));
    }

    #[test]
    fn a_mismatched_migration_name_is_not_ready() {
        let mut mismatched = current_history();
        mismatched[6].migration_name = "0007_different.sql".to_owned();
        let result = assess_migration_history(&mismatched, RELEASE_SCHEMA_REQUIREMENT);
        assert_eq!(
            result.failure,
            Some(ReadinessFailure::MigrationNameMismatch)
        );
    }

    #[test]
    fn an_incompatible_future_history_is_not_ready() {
        let mut incompatible = current_history();
        incompatible.push(MigrationRecord {
            migration_id: 8,
            migration_name: "destructive_cleanup.sql".to_owned(),
        });
        let result = assess_migration_history(&incompatible, RELEASE_SCHEMA_REQUIREMENT);
        assert_eq!(
            result.failure,
            Some(ReadinessFailure::MigrationNameMismatch)
        );
    }
}
