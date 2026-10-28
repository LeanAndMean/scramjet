use crate::{MAX_SYNTHETIC_TOKENS, RequestId};

fn decimal(bytes: &[u8], length: usize) -> Option<u64> {
    if bytes.is_empty() || bytes.len() > length {
        return None;
    }
    bytes.iter().try_fold(0u64, |n, b| {
        if !b.is_ascii_digit() {
            return None;
        }
        n.checked_mul(10)?.checked_add(u64::from(b - b'0'))
    })
}
pub(super) fn parse<'a>(
    ids: impl Iterator<Item = &'a [u8]>,
    values: impl Iterator<Item = &'a [u8]>,
    id: RequestId,
) -> Result<Option<u32>, ()> {
    let mut ids = ids;
    let mut values = values;
    match (ids.next(), values.next()) {
        (None, None) => Ok(None),
        (Some(i), Some(v)) if ids.next().is_none() && values.next().is_none() => {
            let actual = decimal(v, 5)
                .filter(|n| *n <= u64::from(MAX_SYNTHETIC_TOKENS))
                .ok_or(())?;
            if decimal(i, 20) != Some(id.0) {
                return Err(());
            }
            Ok(Some(actual as u32))
        }
        _ => Err(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_correlated_scalar_evidence() {
        let parse_one = |i: &[u8], v: &[u8]| parse([i].into_iter(), [v].into_iter(), RequestId(7));
        for v in [b"0".as_slice(), b"1", b"65536", b"00001"] {
            assert!(parse_one(b"7", v).is_ok());
        }
        for v in [
            b"".as_slice(),
            b"65537",
            b"999999",
            b"+1",
            b"-1",
            b" 1",
            b"1 ",
            b"1,2",
            b"\xff",
            b"18446744073709551616",
        ] {
            assert!(parse_one(b"7", v).is_err());
        }
        for i in [
            b"8".as_slice(),
            b"+7",
            b" 7",
            b"7 ",
            b"18446744073709551616",
            b"000000000000000000007",
        ] {
            assert!(parse_one(i, b"1").is_err());
        }
        assert_eq!(
            parse(std::iter::empty(), std::iter::empty(), RequestId(7)),
            Ok(None)
        );
        assert!(
            parse(
                [b"7".as_slice()].into_iter(),
                std::iter::empty(),
                RequestId(7)
            )
            .is_err()
        );
        assert!(
            parse(
                std::iter::empty(),
                [b"1".as_slice()].into_iter(),
                RequestId(7)
            )
            .is_err()
        );
        assert!(
            parse(
                [b"7".as_slice(); 2].into_iter(),
                [b"1".as_slice()].into_iter(),
                RequestId(7)
            )
            .is_err()
        );
        assert!(
            parse(
                [b"7".as_slice()].into_iter(),
                [b"1".as_slice(); 2].into_iter(),
                RequestId(7)
            )
            .is_err()
        );
    }
}
