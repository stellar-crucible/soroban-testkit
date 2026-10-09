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

#[cfg(test)]
mod tests {
    use super::DecodedError;

    #[test]
    fn display_includes_code_and_message() {
        let err = DecodedError {
            code: 12,
            message: "Insufficient balance".to_string(),
            context: None,
        };
        assert_eq!(err.to_string(), "Soroban Error [12]: Insufficient balance");
    }

    #[test]
    fn display_appends_context_when_present() {
        let err = DecodedError {
            code: 7,
            message: "Not authorized".to_string(),
            context: Some("transfer".to_string()),
        };
        assert_eq!(
            err.to_string(),
            "Soroban Error [7]: Not authorized (context: transfer)"
        );
    }

    #[test]
    fn empty_message_still_renders_the_code() {
        let err = DecodedError {
            code: 0,
            message: String::new(),
            context: None,
        };
        assert_eq!(err.to_string(), "Soroban Error [0]: ");
    }

    #[test]
    fn clone_produces_an_independent_copy() {
        let err = DecodedError {
            code: 1,
            message: "first".to_string(),
            context: Some("ctx".to_string()),
        };
        let mut copy = err.clone();
        copy.message = "second".to_string();

        assert_eq!(err.message, "first");
        assert_eq!(copy.message, "second");
    }
}
