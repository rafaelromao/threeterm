pub mod artifact;
pub mod command_execution;
pub mod diagnostic;
pub mod frame;
pub mod schema;
pub mod schema_validator;
pub mod supervisor;
pub mod worker;

pub fn schema_version() -> &'static str {
    "threeterm.protocol/1"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), "threeterm.protocol/1");
    }
}
