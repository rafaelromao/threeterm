//! Package target for the production adversarial rehearsal harness.

mod reliability;

pub use reliability::{
    RELIABILITY_SCHEMA_VERSION, RELIABILITY_TEST_ID, run_reliability_failure_drill,
    verify_reliability_failure_evidence,
};
