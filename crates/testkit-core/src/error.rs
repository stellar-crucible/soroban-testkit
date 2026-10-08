use std::fmt;

#[derive(Debug, Clone)]
pub struct DecodedError {
    pub code: u32,
    pub message: String,
    pub context: Option<String>,
}

impl fmt::Display for DecodedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Soroban Error [{}]: {}", self.code, self.message)?;
        if let Some(ctx) = &self.context {
            write!(f, " (context: {})", ctx)?;
        }
        Ok(())
    }
}
