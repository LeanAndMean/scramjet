mod provider_diagnostic;
pub(crate) mod responses;
pub(crate) mod retry_after;

/// `error` with its causes after it, as hyper's own text names only the kind of failure
/// ("connection error") and keeps the cause, such as a reset connection, in its source.
pub(crate) fn described(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(error) = cause {
        let more = error.to_string();
        if !more.is_empty() && !text.ends_with(&more) {
            text = format!("{text}: {more}");
        }
        cause = error.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::described;
    use std::io;

    #[derive(Debug)]
    struct Wrapper(&'static str, io::Error);

    impl std::fmt::Display for Wrapper {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(self.0)
        }
    }

    impl std::error::Error for Wrapper {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.1)
        }
    }

    #[test]
    fn an_error_is_described_with_its_causes_each_once() {
        let reset = || io::Error::from(io::ErrorKind::ConnectionReset);
        assert_eq!(
            described(&Wrapper("connection error", reset())),
            "connection error: connection reset"
        );
        assert_eq!(
            described(&Wrapper("failed: connection reset", reset())),
            "failed: connection reset"
        );
        assert_eq!(described(&reset()), "connection reset");
    }
}
