pub fn schema_version() -> &'static str {
    "threeterm.lua-bridge/1"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), "threeterm.lua-bridge/1");
    }
}
