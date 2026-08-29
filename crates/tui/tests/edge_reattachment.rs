use serde_json::json;
use threeterm_tui::reattachment_acknowledgement;

#[test]
fn interactive_overlay_acknowledges_each_structured_reattachment_outcome() {
    assert_eq!(
        reattachment_acknowledgement(&json!({
            "outcome": "resolved",
            "selected_edge_id": "edge-new"
        })),
        "edge reattached: edge-new"
    );
    for outcome in ["ambiguous", "lost", "incompatible"] {
        assert_eq!(
            reattachment_acknowledgement(&json!({"outcome": outcome})),
            format!("edge reattachment {outcome}")
        );
    }
}
