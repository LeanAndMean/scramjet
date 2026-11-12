//! A server's failure text with the credentials its request carried taken out, as an answer may
//! echo a header back (`docs/RPC.md`, section 10).

/// Headers whose values say nothing secret, kept in failures.
const PLAIN: &[&str] = &[
    "accept",
    "anthropic-beta",
    "anthropic-version",
    "content-type",
    "copilot-integration-id",
    "copilot-vision-request",
    "editor-plugin-version",
    "editor-version",
    "http-referer",
    "mcp-protocol-version",
    "mcp-session-id",
    "openai-beta",
    "openai-intent",
    "originator",
    "session_id",
    "user-agent",
    "x-client-request-id",
    "x-github-api-version",
    "x-initiator",
    "x-title",
];

/// Shorter values are left: they could not be a credential, and replacing them would mangle
/// ordinary words.
const SHORTEST: usize = 8;

pub(crate) const REDACTED: &str = "[redacted]";

/// The values of `headers` that may be credentials, longest first: each whole, and after an
/// authorization scheme (`Bearer `) also without it.
pub(crate) fn secrets<'a>(headers: impl IntoIterator<Item = (&'a str, &'a str)>) -> Vec<String> {
    let mut secrets = Vec::new();
    for (name, value) in headers {
        if PLAIN.iter().any(|plain| plain.eq_ignore_ascii_case(name)) {
            continue;
        }
        let value = value.trim();
        secrets.push(value);
        if let Some((scheme, rest)) = value.split_once(' ')
            && scheme.bytes().all(|byte| byte.is_ascii_alphabetic())
        {
            secrets.push(rest.trim());
        }
    }
    secrets.retain(|secret| secret.len() >= SHORTEST);
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets.dedup();
    secrets.into_iter().map(str::to_owned).collect()
}

/// Form fields that say nothing secret, kept in failures: `grant_type=refresh_token` must not
/// take the word out of an `invalid refresh_token` answer.
const PLAIN_FIELDS: &[&str] = &[
    "client_id",
    "code_challenge_method",
    "grant_type",
    "redirect_uri",
    "response_type",
    "scope",
];

/// The values of an `application/x-www-form-urlencoded` body that may be credentials (a refresh
/// token, a device code), as sent and decoded.
pub(crate) fn form_secrets(form: &str) -> Vec<String> {
    let mut secrets = Vec::new();
    for pair in form.split('&') {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if PLAIN_FIELDS.contains(&super::codex::decode(name).as_str()) {
            continue;
        }
        secrets.push(super::codex::decode(value));
        secrets.push(value.to_owned());
    }
    secrets.retain(|secret| secret.len() >= SHORTEST);
    secrets
}

/// `text` with each of `secrets` replaced by `[redacted]`.
pub(crate) fn redact(text: &str, secrets: &[String]) -> String {
    let mut text = text.to_owned();
    for secret in secrets {
        if text.contains(secret.as_str()) {
            text = text.replace(secret.as_str(), REDACTED);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn takes_out_credentials_and_keeps_plain_headers() {
        let secrets = secrets([
            ("Authorization", "Bearer sk-abcdef123456"),
            ("x-api-key", "key-0987654321"),
            ("anthropic-version", "2023-06-01"),
            ("x-short", "abc"),
        ]);
        let text = "bad header Authorization: Bearer sk-abcdef123456, token sk-abcdef123456, \
                    key-0987654321, version 2023-06-01, abc";
        assert_eq!(
            redact(text, &secrets),
            "bad header Authorization: [redacted], token [redacted], [redacted], \
             version 2023-06-01, abc"
        );
    }

    #[test]
    fn takes_out_a_forms_tokens_and_keeps_its_plain_fields() {
        let secrets = form_secrets(
            "grant_type=refresh_token&refresh_token=rt_abc%2Fdef123&client_id=app_0123456789",
        );
        assert_eq!(secrets, ["rt_abc/def123", "rt_abc%2Fdef123"]);
        assert_eq!(
            redact("invalid refresh_token: rt_abc/def123", &secrets),
            "invalid refresh_token: [redacted]"
        );
    }

    #[test]
    fn a_value_with_spaces_after_a_word_that_is_no_scheme_stays_whole() {
        let secrets = secrets([("x-custom", "my-secret-1 and-more")]);
        assert_eq!(secrets, ["my-secret-1 and-more"]);
    }
}
