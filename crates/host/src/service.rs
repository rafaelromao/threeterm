//! Host service that binds the domain graph to the persistence surface.
//!
//! `ProjectService` is the single orchestrator the CLI/MCP/TUI adapters
//! call into. It owns no filesystem state of its own: every operation
//! delegates to `threeterm_persistence::bundle` for the actual reads and
//! writes. The service is intentionally a thin facade so the persistence
//! layer stays the single source of truth for the canonical log identity.

use std::path::Path;

use threeterm_domain::graph::{CommandIntent, ProjectGeneration};
use threeterm_persistence::bundle::{self, BundleError, LoadedBundle, Manifest};

/// Orchestrates the project lifecycle on top of the persistence layer.
#[derive(Debug, Clone, Copy, Default)]
pub struct ProjectService;

impl ProjectService {
    pub const fn new() -> Self {
        Self
    }

    /// Create a fresh project bundle at `path`.
    ///
    /// Returns the `ProjectGeneration` whose `id` is the canonical log
    /// digest at the empty initial state. The empty digest is a fixed,
    /// well-known SHA-256 so two consecutive `new_project` calls produce
    /// byte-equal identities.
    pub fn new_project(&self, path: &Path) -> Result<ProjectGeneration, BundleError> {
        let generation = ProjectGeneration::fresh();
        let manifest = bundle::write_fresh(path, generation)?;
        Ok(project_generation_from_manifest(&manifest))
    }

    /// Apply a command intent to an existing bundle, appending one
    /// transaction to the canonical log.
    ///
    /// Returns the `ProjectGeneration` whose `id` is the canonical log
    /// digest after the append. Reloading the bundle produces the same
    /// digest.
    pub fn apply(
        &self,
        path: &Path,
        intent: &CommandIntent,
    ) -> Result<ProjectGeneration, BundleError> {
        let manifest = bundle::append_transaction(path, intent)?;
        Ok(project_generation_from_manifest(&manifest))
    }

    /// Load the current `ProjectGeneration` from disk.
    pub fn load(&self, path: &Path) -> Result<ProjectGeneration, BundleError> {
        let bundle = bundle::load(path)?;
        Ok(project_generation_from_manifest(&bundle.manifest))
    }

    /// Read the canonical log identity for an existing bundle.
    ///
    /// Equivalent to `load` followed by extracting the `id`, but exposes
    /// the seam explicitly so the CLI's `--machine identity <path>` is
    /// driven by a named host operation.
    pub fn identity(&self, path: &Path) -> Result<ProjectGeneration, BundleError> {
        let bundle = bundle::load(path)?;
        Ok(project_generation_from_manifest(&bundle.manifest))
    }
}

fn project_generation_from_manifest(manifest: &Manifest) -> ProjectGeneration {
    ProjectGeneration::with_id(manifest.log_identity.clone())
}

/// Optional: return the loaded bundle alongside the generation so the
/// CLI/MCP adapters can echo the manifest verbatim. Kept as a free
/// function so it composes with the `*_project` / `apply` / `load` /
/// `identity` methods above.
pub fn load_full(path: &Path) -> Result<LoadedBundle, BundleError> {
    bundle::load(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use threeterm_domain::graph::FeatureId;

    fn fresh_root(tag: &str) -> std::path::PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("threeterm-host-{tag}-{suffix}"));
        let _ = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn new_project_returns_generation_with_empty_log_identity() {
        let root = fresh_root("new");
        let service = ProjectService::new();
        let generation = service
            .new_project(&root)
            .expect("new_project creates bundle");
        let expected = bundle::log_identity_hex(b"");
        assert_eq!(generation.id, expected);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn apply_changes_the_generation_identity_to_match_the_extended_log() {
        let root = fresh_root("apply");
        let service = ProjectService::new();
        let initial = service.new_project(&root).expect("new_project");

        let intent = CommandIntent::AddFeature {
            feature_id: FeatureId::new("sketch-1").unwrap(),
            feature_kind: "sketch".to_string(),
            parameters: serde_json::json!({"plane": "xy"}),
        };
        let after_apply = service.apply(&root, &intent).expect("apply");

        assert_ne!(after_apply.id, initial.id, "apply must change the identity");
        // The identity returned by `apply` must equal what a reload would compute.
        let reloaded = service.load(&root).expect("reload");
        assert_eq!(
            reloaded.id, after_apply.id,
            "identity must be byte-equal across reload"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn identity_command_returns_byte_equal_id_after_reload() {
        let root = fresh_root("identity");
        let service = ProjectService::new();
        let created = service.new_project(&root).expect("new_project");
        let snapshotted = service.identity(&root).expect("identity");
        assert_eq!(created.id, snapshotted.id);

        let intent = CommandIntent::SetParameter {
            feature_id: FeatureId::new("sketch-1").unwrap(),
            parameter: "width".to_string(),
            value: serde_json::json!(2.5),
        };
        let after_apply = service.apply(&root, &intent).expect("apply");
        let snapshotted_after = service.identity(&root).expect("identity after apply");
        assert_eq!(after_apply.id, snapshotted_after.id);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn load_returns_byte_equal_identity_after_apply_sequence() {
        let root = fresh_root("sequence");
        let service = ProjectService::new();
        service.new_project(&root).expect("new_project");

        let intents = [
            CommandIntent::AddFeature {
                feature_id: FeatureId::new("sketch-1").unwrap(),
                feature_kind: "sketch".to_string(),
                parameters: serde_json::json!({"plane": "xy"}),
            },
            CommandIntent::SetParameter {
                feature_id: FeatureId::new("sketch-1").unwrap(),
                parameter: "width".to_string(),
                value: serde_json::json!(10.0),
            },
            CommandIntent::AddFeature {
                feature_id: FeatureId::new("extrude-1").unwrap(),
                feature_kind: "extrude".to_string(),
                parameters: serde_json::json!({"depth": 5.0}),
            },
            CommandIntent::SetParameter {
                feature_id: FeatureId::new("extrude-1").unwrap(),
                parameter: "depth".to_string(),
                value: serde_json::json!(7.5),
            },
            CommandIntent::RemoveFeature {
                feature_id: FeatureId::new("sketch-1").unwrap(),
            },
        ];

        let mut last_id = None;
        for intent in &intents {
            let generation = service.apply(&root, intent).expect("apply");
            last_id = Some(generation.id);
        }

        let final_apply_id = last_id.expect("at least one apply");
        let reloaded = service.load(&root).expect("reload");
        assert_eq!(
            reloaded.id, final_apply_id,
            "final identity must be byte-equal across reload"
        );

        let snapshotted = service.identity(&root).expect("identity after sequence");
        assert_eq!(snapshotted.id, final_apply_id);
        let _ = fs::remove_dir_all(&root);
    }
}
