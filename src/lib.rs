pub fn smoke_message() -> &'static str {
    "scramjet-rs: offline bootstrap ready"
}

#[cfg(test)]
mod tests {
    use super::smoke_message;

    #[test]
    fn smoke_message_identifies_the_offline_bootstrap() {
        assert_eq!(smoke_message(), "scramjet-rs: offline bootstrap ready");
    }
}
