use threeterm_viewport::{
    FrameAcknowledgement, FrameIdentity, RenderCoordinator, Renderer, RendererSubmission,
    ViewportDiagnostic, ViewportDiagnosticCode, ViewportFrame,
};

#[derive(Debug, Default)]
struct RecordingRenderer {
    submissions: Vec<FrameIdentity>,
    acknowledgements: Vec<FrameAcknowledgement>,
}

impl Renderer for RecordingRenderer {
    fn submit_image(
        &mut self,
        frame: &ViewportFrame,
        frame_token: u64,
    ) -> Result<RendererSubmission, ViewportDiagnostic> {
        let identity = FrameIdentity {
            frame_token,
            generation: frame.generation,
            revision: frame.revision.clone(),
            image_id: frame_token + 100,
        };
        self.submissions.push(identity.clone());
        Ok(RendererSubmission { identity })
    }

    fn request_cancel(
        &mut self,
        _active: Option<&FrameIdentity>,
    ) -> Result<(), ViewportDiagnostic> {
        Ok(())
    }

    fn acknowledge(
        &mut self,
        acknowledgement: &FrameAcknowledgement,
    ) -> Result<(), ViewportDiagnostic> {
        self.acknowledgements.push(acknowledgement.clone());
        Ok(())
    }

    fn cleanup(&mut self) -> Result<(), ViewportDiagnostic> {
        Ok(())
    }
}

fn frame(generation: u64) -> ViewportFrame {
    ViewportFrame {
        revision: "revision-a".to_string(),
        generation,
        width: 1,
        height: 1,
        rgb: vec![generation as u8, 0, 0],
        frame_token: None,
    }
}

#[test]
fn coordinator_keeps_one_in_flight_and_only_the_newest_pending_frame() {
    let mut coordinator = RenderCoordinator::new(RecordingRenderer::default());

    let first = coordinator.submit(frame(1)).expect("first frame starts");
    let second = coordinator
        .submit(frame(2))
        .expect("second frame is pending");
    let third = coordinator
        .submit(frame(3))
        .expect("newest frame replaces pending");

    assert!(first.started.is_some());
    assert!(second.started.is_none());
    assert!(third.started.is_none());
    assert_eq!(coordinator.renderer().submissions.len(), 1);
    assert_eq!(coordinator.dropped_frames().len(), 1);
    assert_eq!(coordinator.dropped_frames()[0].generation, 2);

    let first_ack = coordinator
        .acknowledge(FrameAcknowledgement::from(&first.started.unwrap()))
        .expect("first acknowledgement starts the newest pending frame");
    assert_eq!(first_ack.visible.as_ref().unwrap().generation, 1);
    assert_eq!(first_ack.started.as_ref().unwrap().generation, 3);
    assert_eq!(coordinator.renderer().submissions.len(), 2);

    let newest = first_ack.started.unwrap();
    let newest_ack = coordinator
        .acknowledge(FrameAcknowledgement::from(&newest))
        .expect("newest acknowledgement publishes");
    assert_eq!(newest_ack.visible.as_ref().unwrap().generation, 3);
    assert_eq!(coordinator.visible_frame().unwrap().generation, 3);
}

#[test]
fn cancellation_drops_pending_and_prevents_cancelled_frame_publication() {
    let mut coordinator = RenderCoordinator::new(RecordingRenderer::default());
    let first = coordinator.submit(frame(1)).expect("first frame starts");
    coordinator
        .submit(frame(2))
        .expect("second frame is pending");

    let cancelled = coordinator
        .request_cancel()
        .expect("cancellation is accepted");
    assert_eq!(cancelled.cancelled_pending.unwrap().generation, 2);
    assert_eq!(cancelled.active.unwrap().generation, 1);
    let acknowledged = coordinator
        .acknowledge(FrameAcknowledgement::from(&first.started.unwrap()))
        .expect("cancelled in-flight frame can finish its wire lifecycle");
    assert!(acknowledged.visible.is_none());
    assert!(acknowledged.started.is_none());
    assert!(coordinator.visible_frame().is_none());
}

#[test]
fn invalidated_attachment_rejects_new_frames_with_structured_state() {
    let mut coordinator = RenderCoordinator::new(RecordingRenderer::default());
    coordinator.invalidate();

    let diagnostic = coordinator
        .submit(frame(1))
        .expect_err("invalidated attachments cannot accept new frames");
    assert_eq!(
        diagnostic.code,
        ViewportDiagnosticCode::CapabilityInvalidated
    );
    assert!(!coordinator.is_valid());
}
