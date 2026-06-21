use smol_workflow_sandbox::ProviderError;

pub type ProviderResult<T> = Result<T, ProviderError>;

pub fn provider_error(
    code: impl Into<String>,
    message: impl Into<String>,
    retryable: bool,
) -> ProviderError {
    ProviderError {
        code: code.into(),
        message: message.into(),
        retryable,
    }
}

pub fn bad_profile(message: impl Into<String>) -> ProviderError {
    provider_error("bad_profile", message, false)
}

pub fn provider_failure(message: impl Into<String>) -> ProviderError {
    provider_error("provider_failure", message, false)
}

pub fn unsupported_method(method: &str) -> ProviderError {
    provider_error(
        "unsupported_method",
        format!("method `{method}` is not implemented by the exe.dev provider"),
        false,
    )
}

pub fn invalid_request(message: impl Into<String>) -> ProviderError {
    provider_error("invalid_request", message, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_builds_provider_error() {
        let error = provider_error("ssh_not_ready", "not ready", true);
        assert_eq!(error.code, "ssh_not_ready");
        assert_eq!(error.message, "not ready");
        assert!(error.retryable);
    }
}
