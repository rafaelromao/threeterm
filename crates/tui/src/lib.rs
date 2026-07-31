pub fn schema_version() -> &'static str {
    "threeterm.tui/1"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), "threeterm.tui/1");
    }
}
