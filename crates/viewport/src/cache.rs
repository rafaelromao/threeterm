use std::collections::HashMap;

use threeterm_protocol::artifact::WorkerFingerprint;

use crate::diagnostic::ViewportDiagnostic;
use crate::projection::{CameraState, ViewportFrame, ViewportRequest, ViewportScene};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PreviewScope {
    pub command_schema: String,
    pub input_fingerprint: String,
}

impl PreviewScope {
    pub fn new(command_schema: impl Into<String>, input_fingerprint: impl Into<String>) -> Self {
        Self {
            command_schema: command_schema.into(),
            input_fingerprint: input_fingerprint.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    revision: String,
    worker_fingerprint: WorkerFingerprint,
    layer1_reference: String,
    width: u32,
    height: u32,
    frustum_band: u8,
    quality_level: u8,
    selection_fingerprint: String,
    preview_scope: Option<PreviewScope>,
}

/// Host-only, in-memory, per-session viewport display cache (Layer 2).
///
/// Keyed by the 8-tuple from issue #37 / #246. Never persisted, never
/// crosses IPC, disposable on revision transition. Navigation never
/// produces Layer 1 work — cache only stores frames derived from an
/// existing Derived Result.
#[derive(Debug, Default)]
pub struct ViewportDisplayCache {
    entries: HashMap<CacheKey, ViewportFrame>,
}

impl ViewportDisplayCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn contains(
        &self,
        revision: &str,
        worker_fingerprint: &WorkerFingerprint,
        layer1_reference: &str,
        width: u32,
        height: u32,
        frustum_band: u8,
        quality_level: u8,
        selection_fingerprint: &str,
        preview_scope: Option<&PreviewScope>,
    ) -> bool {
        let key = CacheKey {
            revision: revision.to_string(),
            worker_fingerprint: worker_fingerprint.clone(),
            layer1_reference: layer1_reference.to_string(),
            width,
            height,
            frustum_band,
            quality_level,
            selection_fingerprint: selection_fingerprint.to_string(),
            preview_scope: preview_scope.cloned(),
        };
        self.entries.contains_key(&key)
    }

    /// Invalidate preview-only entries for the given scope. Called when a
    /// preview session ends (cancel/commit/expiry) so preview geometry does
    /// not leak beyond the session.
    pub fn invalidate_preview_scope(&mut self, scope: &PreviewScope) {
        self.entries
            .retain(|key, _| key.preview_scope.as_ref() != Some(scope));
    }

    /// Clear all entries (e.g. on revision transition when Host advances).
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Returns true if the entry must never be cached per the exclusion policy.
    /// Exclusions: Command Drafts, hover/pointer/candidate, stale last-valid
    /// geometry, preview-only beyond session (handled via invalidate), worker
    /// internals (temp paths / stderr markers).
    pub fn is_excluded(
        &self,
        layer1_reference: &str,
        preview_scope: Option<&PreviewScope>,
    ) -> bool {
        if layer1_reference.is_empty() {
            return true;
        }
        let lower = layer1_reference.to_ascii_lowercase();
        if lower.contains("draft")
            || lower.contains("hover")
            || lower.contains("candidate")
            || lower.contains("pointer")
            || lower.contains("stale")
            || lower.contains("preview-only")
            || lower.contains("worker-internal")
            || lower.contains("tmp/")
            || lower.contains("stderr")
        {
            return true;
        }
        if let Some(scope) = preview_scope {
            let s = scope.command_schema.to_ascii_lowercase();
            if s.contains("draft") {
                return true;
            }
            let f = scope.input_fingerprint.to_ascii_lowercase();
            if f.contains("draft") || f.contains("hover") || f.contains("stale") {
                return true;
            }
        }
        false
    }

    /// Get cached frame or compute it via `projection_fn`.
    ///
    /// On cache hit returns the previously computed frame without calling
    /// `projection_fn`. On miss calls `projection_fn`; on success stores the
    /// frame unless excluded; on diagnostic error never stores and propagates
    /// the diagnostic so canonical Host state is preserved.
    #[allow(clippy::too_many_arguments)]
    pub fn get_or_project<F>(
        &mut self,
        scene: &ViewportScene,
        request: ViewportRequest,
        worker_fingerprint: &WorkerFingerprint,
        layer1_reference: &str,
        quality_level: u8,
        selection_fingerprint: String,
        preview_scope: Option<PreviewScope>,
        projection_fn: F,
    ) -> Result<(ViewportFrame, bool), ViewportDiagnostic>
    where
        F: FnOnce(&ViewportScene, ViewportRequest) -> Result<ViewportFrame, ViewportDiagnostic>,
    {
        // Exclusions are not inserted — compute without caching (but don't treat as hit).
        if self.is_excluded(layer1_reference, preview_scope.as_ref()) {
            let frame = projection_fn(scene, request)?;
            return Ok((frame, false));
        }

        let frustum_band = frustum_band_from_camera(&request.camera);
        let key = CacheKey {
            revision: request.revision.clone(),
            worker_fingerprint: worker_fingerprint.clone(),
            layer1_reference: layer1_reference.to_string(),
            width: request.width,
            height: request.height,
            frustum_band,
            quality_level,
            selection_fingerprint: selection_fingerprint.clone(),
            preview_scope: preview_scope.clone(),
        };

        if let Some(cached) = self.entries.get(&key) {
            return Ok((cached.clone(), true));
        }

        let frame = projection_fn(scene, request)?;
        // Store only on successful projection
        self.entries.insert(key, frame.clone());
        Ok((frame, false))
    }
}

/// Quantize CameraState into a single frustum band byte.
///
/// Yaw normalized to 0..360 -> 24 bands (15 deg each), pitch -89..89 ->
/// ~12 bands. Combined into one byte for the cache key so that small
/// navigational jitter within one band reuses the same derived result.
pub fn frustum_band_from_camera(camera: &CameraState) -> u8 {
    let yaw_band = (camera.yaw_degrees.rem_euclid(360) / 15) as u8;
    let pitch_band = ((camera.pitch_degrees + 89) / 15).clamp(0, 11) as u8;
    yaw_band.wrapping_mul(13).wrapping_add(pitch_band)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projection::{CameraState, ProtocolNeutralViewport, SceneFeature, ViewportScene};

    fn fingerprint() -> WorkerFingerprint {
        WorkerFingerprint {
            worker_kind: "occt".to_string(),
            worker_schema_version: "threeterm.workers.occt/1".to_string(),
            protocol_schema_version: "threeterm.protocol/1".to_string(),
        }
    }

    fn scene(revision: &str) -> ViewportScene {
        ViewportScene {
            revision: revision.to_string(),
            features: vec![SceneFeature {
                id: "f1".to_string(),
                kind: "plate-vertical".to_string(),
            }],
            selected_id: None,
            layer1_references: vec!["derived-abc".to_string()],
        }
    }

    #[test]
    fn orbit_ten_frames_one_miss_nine_hits() {
        let mut cache = ViewportDisplayCache::new();
        let fp = fingerprint();
        let sc = scene("rev-1");
        let base = CameraState::new(0, 0, 100);
        let mut invocations = 0usize;
        for i in 0..10 {
            let camera = base.rotated(i, 0); // stays within one 15-deg band for i=0..9 when starting at 0? 0..9 stays in band 0
            // Ensure within same band: 0..9 all in yaw band 0 (0..14)
            let req = crate::projection::ViewportRequest::new("rev-1", i as u64, 80, 24, camera);
            let (frame, hit) = cache
                .get_or_project(
                    &sc,
                    req.clone(),
                    &fp,
                    "derived-abc",
                    0,
                    "".to_string(),
                    None,
                    |s, r| {
                        invocations += 1;
                        ProtocolNeutralViewport::project(s, r)
                    },
                )
                .expect("project succeeds");
            assert_eq!(frame.revision, "rev-1");
            if i == 0 {
                assert!(!hit, "first frame must be miss");
            } else {
                assert!(hit, "orbit within same band must hit at frame {i}");
            }
        }
        assert_eq!(invocations, 1, "OCCT projection invoked once, 9 hits");
        assert_eq!(cache.len(), 1);
    }
}
