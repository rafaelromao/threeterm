use threeterm_domain::{
    CommandIntent, CommandTransaction, DomainError, ProjectGeneration, Revision,
};

pub mod service {
    pub use super::ProjectService;
}

pub fn schema_version() -> &'static str {
    "threeterm.host/1"
}

#[derive(Debug, Clone)]
pub struct ProjectService {
    generation: ProjectGeneration,
}

impl ProjectService {
    pub fn new(generation: ProjectGeneration) -> Self {
        Self { generation }
    }

    pub fn current_revision(&self) -> &Revision {
        self.generation.current_revision()
    }

    pub fn execute(&mut self, intent: CommandIntent) -> Result<CommandTransaction, DomainError> {
        let mut next = self.generation.clone();
        let transaction = next.apply(intent)?;
        self.generation = next;
        Ok(transaction)
    }

    pub fn generation(&self) -> &ProjectGeneration {
        &self.generation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_version_matches_pinned_string() {
        assert_eq!(schema_version(), "threeterm.host/1");
    }
}
