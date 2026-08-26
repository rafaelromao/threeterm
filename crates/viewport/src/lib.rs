#![allow(clippy::result_large_err)]

mod cache;
mod capability;
mod diagnostic;
mod kitty;
mod projection;
mod renderer;
mod signal;

pub use cache::{
    InvalidationOutcome, InvalidationTrigger, PreviewScope, ViewportDisplayCache,
    frustum_band_from_camera,
};
pub use capability::{
    CapabilityProbe, CapabilityProbeIo, CapabilityProbeResult, CapabilityState,
    CapabilityTranscript, MAX_PROBE_RESPONSE_BYTES, TerminalCapabilityVector, TerminalEnvironment,
};
pub use diagnostic::{SCHEMA_VERSION, ViewportDiagnostic, ViewportDiagnosticCode};
pub use kitty::{
    GhosttyRenderer, KittyPlacement, MAX_BASE64_CHUNK, MAX_COMPRESSED_PAYLOAD, NoopTermiosRestorer,
    TermiosRestorer, parse_ack,
};
pub use projection::{
    CameraState, MAX_PIXELS, ProtocolNeutralViewport, SceneFeature, SceneSolid, SceneTriangle,
    ViewportFrame, ViewportRequest, ViewportScene,
};
pub use renderer::{
    AcknowledgeOutcome, CancelOutcome, FrameAcknowledgement, FrameIdentity, RenderCoordinator,
    Renderer, RendererSubmission, SubmitOutcome,
};
pub use signal::{CleanupSignal, cleanup_coordinator_on_signal, cleanup_on_signal};

pub fn schema_version() -> &'static str {
    SCHEMA_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), "threeterm.viewport/1");
    }
}
