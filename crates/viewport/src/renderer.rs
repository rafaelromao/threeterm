use crate::diagnostic::{ViewportDiagnostic, ViewportDiagnosticCode};
use crate::projection::ViewportFrame;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameIdentity {
    pub frame_token: u64,
    pub generation: u64,
    pub revision: String,
    pub image_id: u64,
}

impl FrameIdentity {
    fn pending(frame: &ViewportFrame, frame_token: u64) -> Self {
        Self {
            frame_token,
            generation: frame.generation,
            revision: frame.revision.clone(),
            image_id: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RendererSubmission {
    pub identity: FrameIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameAcknowledgement {
    pub frame_token: u64,
    pub image_id: u64,
}

impl From<&FrameIdentity> for FrameAcknowledgement {
    fn from(identity: &FrameIdentity) -> Self {
        Self {
            frame_token: identity.frame_token,
            image_id: identity.image_id,
        }
    }
}

/// Protocol-neutral terminal renderer boundary.
pub trait Renderer {
    fn is_admitted(&self) -> bool {
        true
    }

    fn submit_image(
        &mut self,
        frame: &ViewportFrame,
        frame_token: u64,
    ) -> Result<RendererSubmission, ViewportDiagnostic>;

    fn request_cancel(&mut self, active: Option<&FrameIdentity>) -> Result<(), ViewportDiagnostic>;

    fn acknowledge(
        &mut self,
        acknowledgement: &FrameAcknowledgement,
    ) -> Result<(), ViewportDiagnostic>;

    fn cleanup(&mut self) -> Result<(), ViewportDiagnostic>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitOutcome {
    pub started: Option<FrameIdentity>,
    pub queued: Option<FrameIdentity>,
    pub replaced: Option<FrameIdentity>,
    pub dropped: Option<ViewportDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcknowledgeOutcome {
    pub visible: Option<ViewportFrame>,
    pub started: Option<FrameIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelOutcome {
    pub cancelled_pending: Option<FrameIdentity>,
    pub active: Option<FrameIdentity>,
}

#[derive(Debug)]
struct InFlight {
    frame: ViewportFrame,
    identity: FrameIdentity,
    cancelled: bool,
}

#[derive(Debug)]
struct Pending {
    frame: ViewportFrame,
    identity: FrameIdentity,
}

#[derive(Debug)]
pub struct RenderCoordinator<R> {
    renderer: R,
    next_frame_token: u64,
    in_flight: Option<InFlight>,
    pending: Option<Pending>,
    visible: Option<ViewportFrame>,
    dropped_frames: Vec<FrameIdentity>,
    valid: bool,
}

impl<R: Renderer> RenderCoordinator<R> {
    pub fn new(renderer: R) -> Self {
        Self {
            renderer,
            next_frame_token: 0,
            in_flight: None,
            pending: None,
            visible: None,
            dropped_frames: Vec::new(),
            valid: true,
        }
    }

    pub fn renderer(&self) -> &R {
        &self.renderer
    }

    pub fn renderer_mut(&mut self) -> &mut R {
        &mut self.renderer
    }

    pub fn into_renderer(self) -> R {
        self.renderer
    }

    pub fn submit(&mut self, frame: ViewportFrame) -> Result<SubmitOutcome, ViewportDiagnostic> {
        if !self.valid {
            return Err(ViewportDiagnostic::new(
                ViewportDiagnosticCode::CapabilityInvalidated,
                "renderer attachment is invalid",
                &frame.revision,
                "run a fresh capability probe before submitting a frame",
            )
            .with_generation(frame.generation));
        }
        self.next_frame_token = self.next_frame_token.checked_add(1).ok_or_else(|| {
            self.valid = false;
            ViewportDiagnostic::new(
                ViewportDiagnosticCode::TransportWriteFailed,
                "renderer frame token exhausted",
                &frame.revision,
                "restart the interactive attachment",
            )
            .with_generation(frame.generation)
        })?;
        let frame_token = self.next_frame_token;
        let frame = frame.with_frame_token(frame_token);
        let identity = FrameIdentity::pending(&frame, frame_token);

        if self.in_flight.is_some() {
            let replaced = self.pending.replace(Pending {
                frame,
                identity: identity.clone(),
            });
            let dropped = replaced.as_ref().map(|pending| pending.identity.clone());
            if let Some(identity) = &dropped {
                self.dropped_frames.push(identity.clone());
            }
            let dropped_diagnostic = dropped.as_ref().map(|identity| {
                ViewportDiagnostic::new(
                    ViewportDiagnosticCode::FrameDropped,
                    "pending viewport frame was replaced by a newer state",
                    &identity.revision,
                    "continue with the newest pending presentation",
                )
                .with_frame_token(identity.frame_token)
                .with_generation(identity.generation)
            });
            return Ok(SubmitOutcome {
                started: None,
                queued: Some(identity),
                replaced: dropped,
                dropped: dropped_diagnostic,
            });
        }

        let started = match self.start(frame) {
            Ok(started) => started,
            Err(error) => {
                self.invalidate_and_cleanup();
                return Err(error);
            }
        };
        Ok(SubmitOutcome {
            started: Some(started),
            queued: None,
            replaced: None,
            dropped: None,
        })
    }

    pub fn acknowledge(
        &mut self,
        acknowledgement: FrameAcknowledgement,
    ) -> Result<AcknowledgeOutcome, ViewportDiagnostic> {
        let Some(active) = self.in_flight.as_ref() else {
            let diagnostic = ViewportDiagnostic::new(
                ViewportDiagnosticCode::AcknowledgementMismatch,
                "acknowledgement arrived with no in-flight frame",
                "unknown",
                "discard the late acknowledgement and await a current frame",
            )
            .with_frame_token(acknowledgement.frame_token)
            .with_image_id(acknowledgement.image_id);
            self.invalidate_and_cleanup();
            return Err(diagnostic);
        };
        if active.identity.frame_token != acknowledgement.frame_token
            || active.identity.image_id != acknowledgement.image_id
        {
            let diagnostic = ViewportDiagnostic::new(
                ViewportDiagnosticCode::AcknowledgementMismatch,
                "acknowledgement does not match the in-flight frame",
                &active.identity.revision,
                "discard the acknowledgement and retain the current frame",
            )
            .with_frame_token(acknowledgement.frame_token)
            .with_generation(active.identity.generation)
            .with_image_id(acknowledgement.image_id);
            self.invalidate_and_cleanup();
            return Err(diagnostic);
        }

        if let Err(error) = self.renderer.acknowledge(&acknowledgement) {
            self.invalidate_and_cleanup();
            return Err(error);
        }
        let active = self
            .in_flight
            .take()
            .expect("in-flight frame was checked above");
        let visible = (!active.cancelled).then_some(active.frame);
        if let Some(frame) = &visible {
            self.visible = Some(frame.clone());
        }

        let started = if active.cancelled {
            self.pending = None;
            None
        } else if let Some(pending) = self.pending.take() {
            match self.start(pending.frame) {
                Ok(started) => Some(started),
                Err(error) => {
                    self.invalidate_and_cleanup();
                    return Err(error);
                }
            }
        } else {
            None
        };
        Ok(AcknowledgeOutcome { visible, started })
    }

    pub fn request_cancel(&mut self) -> Result<CancelOutcome, ViewportDiagnostic> {
        let cancelled_pending = self.pending.take().map(|pending| {
            self.dropped_frames.push(pending.identity.clone());
            pending.identity
        });
        let active = self.in_flight.as_mut().map(|in_flight| {
            in_flight.cancelled = true;
            in_flight.identity.clone()
        });
        if let Err(error) = self.renderer.request_cancel(active.as_ref()) {
            self.invalidate_and_cleanup();
            return Err(error);
        }
        Ok(CancelOutcome {
            cancelled_pending,
            active,
        })
    }

    pub fn cleanup(&mut self) -> Result<(), ViewportDiagnostic> {
        self.valid = false;
        self.pending = None;
        self.in_flight = None;
        self.visible = None;
        self.renderer.cleanup()
    }

    pub fn invalidate(&mut self) {
        self.invalidate_and_cleanup();
    }

    fn invalidate_and_cleanup(&mut self) {
        self.valid = false;
        let active = self
            .in_flight
            .as_ref()
            .map(|in_flight| in_flight.identity.clone());
        let _ = self.renderer.request_cancel(active.as_ref());
        let _ = self.renderer.cleanup();
        self.pending = None;
        self.in_flight = None;
        self.visible = None;
    }

    pub fn is_valid(&self) -> bool {
        self.valid
    }

    pub fn visible_frame(&self) -> Option<&ViewportFrame> {
        self.visible.as_ref()
    }

    pub fn in_flight(&self) -> Option<&FrameIdentity> {
        self.in_flight.as_ref().map(|in_flight| &in_flight.identity)
    }

    pub fn pending(&self) -> Option<&FrameIdentity> {
        self.pending.as_ref().map(|pending| &pending.identity)
    }

    pub fn dropped_frames(&self) -> &[FrameIdentity] {
        &self.dropped_frames
    }

    fn start(&mut self, frame: ViewportFrame) -> Result<FrameIdentity, ViewportDiagnostic> {
        let frame_token = frame
            .frame_token
            .expect("coordinator assigns a token before starting a frame");
        let submission = match self.renderer.submit_image(&frame, frame_token) {
            Ok(submission) => submission,
            Err(error) => {
                self.valid = false;
                return Err(error);
            }
        };
        if submission.identity.frame_token != frame_token || submission.identity.image_id == 0 {
            return Err(ViewportDiagnostic::new(
                ViewportDiagnosticCode::ProjectionFailed,
                "renderer returned an invalid frame identity",
                &frame.revision,
                "discard the disposable frame and retry from the canonical projection",
            )
            .with_frame_token(frame_token)
            .with_generation(frame.generation));
        }
        self.in_flight = Some(InFlight {
            frame,
            identity: submission.identity.clone(),
            cancelled: false,
        });
        Ok(submission.identity)
    }
}
