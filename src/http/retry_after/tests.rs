use super::*;

fn at(date: &str) -> SystemTime {
    httpdate::parse_http_date(date).unwrap()
}
fn wait(values: &[&str], now: SystemTime) -> Option<RetryWait> {
    parse(values.iter().map(|s| s.as_bytes()), now)
}

#[test]
fn decimals_duplicates_and_bounded_malformed_metadata() {
    let now = UNIX_EPOCH;
    assert_eq!(wait(&[], now), None);
    for text in ["0", "000", "\t 0 \t"] {
        assert_eq!(wait(&[text], now), Some(RetryWait::Delay(Duration::ZERO)));
    }
    assert_eq!(
        wait(&["18446744073709551615"], now),
        Some(RetryWait::Delay(Duration::from_secs(u64::MAX)))
    );
    assert_eq!(
        wait(&["18446744073709551616"], now),
        Some(RetryWait::Unrepresentable)
    );
    for text in [
        "", " ", "+1", "-1", "1.0", "1 0", "1, 2", "١", "1\n", "\x0b1", "1\x0c",
    ] {
        assert_eq!(wait(&[text], now), None, "{text:?}");
    }
    assert_eq!(
        wait(&["3", "bad", "0009", "4"], now),
        Some(RetryWait::Delay(Duration::from_secs(9)))
    );
    assert_eq!(
        wait(&["3", "18446744073709551616", "bad"], now),
        Some(RetryWait::Unrepresentable)
    );
    let long = [b'9'; 3900];
    assert_eq!(
        parse([long.as_slice()].into_iter(), now),
        Some(RetryWait::Unrepresentable)
    );
    let malformed = [b'x'; 3900];
    assert_eq!(parse([malformed.as_slice()].into_iter(), now), None);
}

#[test]
fn all_date_forms_conversion_and_exact_grammar() {
    let now = at("Sun, 06 Nov 1994 08:49:36 GMT") + Duration::from_millis(250);
    for date in [
        "Sun, 06 Nov 1994 08:49:37 GMT",
        "Sunday, 06-Nov-94 08:49:37 GMT",
        "Sun Nov  6 08:49:37 1994",
    ] {
        assert_eq!(
            wait(&[date], now),
            Some(RetryWait::Delay(Duration::from_millis(750)))
        );
        assert_eq!(
            wait(&[date], now + Duration::from_secs(1)),
            Some(RetryWait::Delay(Duration::ZERO))
        );
    }
    for date in [
        "Mon, 06 Nov 1994 08:49:37 GMT",
        "sun, 06 Nov 1994 08:49:37 GMT",
        "Sun, 06 nov 1994 08:49:37 GMT",
        "Sun, 06 Nov 1994 08:49:37 UTC",
        "Sun, 6 Nov 1994 08:49:37 GMT",
        "Sun, 06 Nov 1994 08:49:37 GMT, 1",
        "Sun, 06 Nov 1994 08:49:37 GMT\n",
        "Sun Nov   6 08:49:37 1994",
    ] {
        assert_eq!(wait(&[date], now), None, "{date}");
    }
    assert_eq!(
        wait(&["Thu, 01 Jan 1970 00:00:00 GMT"], UNIX_EPOCH),
        Some(RetryWait::Delay(Duration::ZERO))
    );
    assert!(matches!(
        wait(&["Fri, 31 Dec 9999 23:59:59 GMT"], now),
        Some(RetryWait::Delay(_))
    ));
    assert_eq!(
        wait(&["Wed, 31 Dec 1969 23:59:59 GMT"], now),
        Some(RetryWait::Unrepresentable)
    );
    assert_eq!(wait(&["Thu, 31 Feb 1969 23:59:59 GMT"], now), None);
    assert_eq!(wait(&["Mon, 29 Feb 1900 00:00:00 GMT"], now), None);
    assert_eq!(wait(&["Wed, 01 Jan 0000 00:00:00 GMT"], now), None);
    assert_eq!(
        wait(
            &["Sun, 06 Nov 1994 08:49:37 GMT"],
            UNIX_EPOCH - Duration::from_secs(1)
        ),
        Some(RetryWait::Unrepresentable)
    );
    let beyond = at("Fri, 31 Dec 9999 23:59:59 GMT") + Duration::from_secs(1);
    assert_eq!(
        wait(&["0", "Sun, 06 Nov 1994 08:49:37 GMT"], beyond),
        Some(RetryWait::Unrepresentable)
    );
}

#[test]
fn leap_calendar_and_rolling_fifty_year_instant() {
    let now = at("Tue, 29 Feb 2000 00:00:00 GMT");
    assert_eq!(
        wait(&["Tue, 29 Feb 2000 00:00:01 GMT"], now),
        Some(RetryWait::Delay(Duration::from_secs(1)))
    );
    for invalid in [
        "Mon, 29 Feb 2100 00:00:00 GMT",
        "Sun, 31 Apr 2000 00:00:00 GMT",
        "Tue, 29 Feb 2000 24:00:00 GMT",
        "Tue, 29 Feb 2000 00:60:00 GMT",
        "Tue, 29 Feb 2000 00:00:61 GMT",
    ] {
        assert_eq!(wait(&[invalid], now), None);
    }
    let now = at("Sat, 31 Dec 2016 23:59:59 GMT") + Duration::from_millis(900);
    assert_eq!(
        wait(&["Sat, 31 Dec 2016 23:59:60 GMT"], now),
        Some(RetryWait::Delay(Duration::from_millis(100)))
    );
    let now = at("Mon, 06 Nov 2028 08:49:37 GMT");
    // Exactly fifty years is future; the following second rolls back a century.
    let future = at("Sun, 06 Nov 2078 08:49:37 GMT");
    assert_eq!(
        wait(&["Sunday, 06-Nov-78 08:49:37 GMT"], now),
        Some(RetryWait::Delay(future.duration_since(now).unwrap()))
    );
    assert_eq!(
        wait(&["Monday, 06-Nov-78 08:49:38 GMT"], now),
        Some(RetryWait::Delay(Duration::ZERO))
    );
    assert_eq!(wait(&["Sunday, 06-Nov-78 08:49:38 GMT"], now), None);
    assert_eq!(
        wait(
            &["Sunday, 06-Nov-78 08:49:37 GMT"],
            now + Duration::from_millis(500)
        ),
        Some(RetryWait::Delay(
            future
                .duration_since(now + Duration::from_millis(500))
                .unwrap()
        ))
    );
}
