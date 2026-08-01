use std::path::Path;

use threeterm_protocol::artifact::{
    ArtifactError, ArtifactHeader, Layer1ArtifactRequest, Layer1CacheKey, Stage, WorkerFingerprint,
};
use threeterm_protocol::worker::Envelope;

pub fn schema_version() -> &'static str {
    "threeterm.workers.occt/1"
}

pub fn worker_fingerprint() -> WorkerFingerprint {
    WorkerFingerprint {
        worker_kind: "occt".to_string(),
        worker_schema_version: schema_version().to_string(),
        protocol_schema_version: threeterm_protocol::schema_version().to_string(),
    }
}

pub fn emit_staged_artifact(
    artifact_root: impl AsRef<Path>,
    request: &Layer1ArtifactRequest,
    bytes: &[u8],
) -> Result<Envelope, ArtifactError> {
    let stage = Stage::open(artifact_root.as_ref())?;
    let staged = stage.stage_bytes(&request.staging_name, bytes)?;
    let worker_fingerprint = worker_fingerprint();
    let cache_key = Layer1CacheKey::issue(request, &worker_fingerprint);
    Ok(Envelope::Artifact {
        schema_version: threeterm_protocol::schema_version().to_string(),
        header: Box::new(ArtifactHeader {
            request_id: request.request_id.clone(),
            source_revision_id: request.source_revision_id.clone(),
            cache_key,
            worker_fingerprint,
            artifact_kind: request.artifact_kind.clone(),
            staging_name: staged.staging_name,
            byte_count: staged.byte_count,
            sha256: staged.sha256,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), "threeterm.workers.occt/1");
    }
}
