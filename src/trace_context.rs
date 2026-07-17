use std::collections::HashSet;

use crate::TraceContext;

/// Parses a single W3C `traceparent` value using strict lowercase framing.
#[must_use]
pub fn parse_traceparent(value: &str) -> Option<TraceContext> {
    let bytes = value.as_bytes();
    if !(55..=512).contains(&bytes.len())
        || bytes[2] != b'-'
        || bytes[35] != b'-'
        || bytes[52] != b'-'
        || !is_lower_hex(&bytes[0..2])
        || !is_lower_hex(&bytes[3..35])
        || !is_lower_hex(&bytes[36..52])
        || !is_lower_hex(&bytes[53..55])
        || &bytes[0..2] == b"ff"
        || bytes[3..35].iter().all(|byte| *byte == b'0')
        || bytes[36..52].iter().all(|byte| *byte == b'0')
    {
        return None;
    }

    if &bytes[0..2] == b"00" {
        if bytes.len() != 55 {
            return None;
        }
    } else if bytes.len() > 55
        && (bytes[55] != b'-'
            || bytes.len() == 56
            || bytes[56..].iter().any(|byte| !byte.is_ascii_graphic()))
    {
        return None;
    }

    let flags = decode_hex_byte(bytes[53], bytes[54]);
    Some(TraceContext::new(
        value[3..35].to_owned(),
        value[36..52].to_owned(),
        flags,
        value.to_owned(),
    ))
}

/// Validates and combines W3C `tracestate` header values in wire order.
#[must_use]
pub fn parse_tracestate<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let combined = values.into_iter().collect::<Vec<_>>().join(",");
    if combined.is_empty() || combined.len() > 512 {
        return None;
    }

    let mut seen = HashSet::new();
    let members = combined
        .split(',')
        .map(|member| member.trim_matches([' ', '\t']))
        .filter(|member| !member.is_empty())
        .collect::<Vec<_>>();
    if members.is_empty() {
        return None;
    }
    if members.len() > 32 {
        return None;
    }

    for member in &members {
        let (key, value) = member.split_once('=')?;
        if value.contains('=')
            || !is_valid_tracestate_key(key)
            || !is_valid_tracestate_value(value)
            || !seen.insert(key)
        {
            return None;
        }
    }

    Some(members.join(","))
}

fn is_lower_hex(value: &[u8]) -> bool {
    value
        .iter()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

fn decode_hex_byte(high: u8, low: u8) -> u8 {
    (hex_nibble(high) << 4) | hex_nibble(low)
}

fn hex_nibble(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        _ => 0,
    }
}

fn is_valid_tracestate_key(key: &str) -> bool {
    if key.is_empty() || key.len() > 256 || !key.is_ascii() {
        return false;
    }

    if let Some((tenant, system)) = key.split_once('@') {
        !tenant.is_empty()
            && tenant.len() <= 241
            && (tenant.as_bytes()[0].is_ascii_lowercase() || tenant.as_bytes()[0].is_ascii_digit())
            && tenant.bytes().all(is_key_char)
            && !system.is_empty()
            && system.len() <= 14
            && system.as_bytes()[0].is_ascii_lowercase()
            && system.bytes().all(is_key_char)
            && !system.contains('@')
    } else {
        key.as_bytes()[0].is_ascii_lowercase() && key.bytes().all(is_key_char)
    }
}

fn is_key_char(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-' | b'*' | b'/')
}

fn is_valid_tracestate_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value.ends_with(' ')
        && value
            .bytes()
            .all(|byte| (0x20..=0x7e).contains(&byte) && !matches!(byte, b',' | b'='))
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use proptest::prelude::*;

    use super::{parse_traceparent, parse_tracestate};

    const VALID: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn parses_valid_context_and_sampled_bit() {
        let parsed = parse_traceparent(VALID).expect("valid traceparent");
        assert_eq!(parsed.trace_id(), "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(parsed.parent_id(), "00f067aa0ba902b7");
        assert_eq!(parsed.flags(), 1);
        assert!(parsed.sampled());
        assert_eq!(parsed.traceparent(), VALID);
    }

    #[test]
    fn sampled_uses_only_the_low_bit() {
        let flags_02 = parse_traceparent(&VALID.replace("-01", "-02")).expect("flags 02");
        assert_eq!(flags_02.flags(), 2);
        assert!(!flags_02.sampled());
        let flags_af = parse_traceparent(&VALID.replace("-01", "-af")).expect("flags af");
        assert_eq!(flags_af.flags(), 0xaf);
        assert!(flags_af.sampled());
    }

    #[test]
    fn rejects_bad_framing_case_zero_ids_and_v00_extensions() {
        for invalid in [
            "not-a-traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra",
            "FF-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-4BF92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-00000000000000000000000000000000-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736_00f067aa0ba902b7-01",
            "00_4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7_01",
            "0A-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00F067aa0ba902b7-01",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-0A",
            "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        ] {
            assert!(parse_traceparent(invalid).is_none(), "accepted {invalid}");
        }
    }

    #[test]
    fn accepts_future_version_with_well_framed_extension() {
        assert!(parse_traceparent(&VALID.replacen("00-", "01-", 1)).is_some());
        let value = VALID.replacen("00-", "01-", 1) + "-vendor";
        assert!(parse_traceparent(&value).is_some());
    }

    #[test]
    fn rejects_invalid_future_extensions_and_oversized_traceparent() {
        let future = VALID.replacen("00-", "01-", 1);
        for invalid in [
            format!("{future}-"),
            format!("{future}vendor"),
            format!("{future}-vendor space"),
            format!("{future}-{}", "x".repeat(512)),
        ] {
            assert!(
                parse_traceparent(&invalid).is_none(),
                "accepted {invalid:?}"
            );
        }
    }

    #[test]
    fn combines_valid_tracestate_in_wire_order() {
        assert_eq!(
            parse_tracestate(["vendor=value", "tenant@system=opaque"]),
            Some("vendor=value,tenant@system=opaque".to_owned())
        );
        let trace = parse_traceparent(VALID)
            .expect("trace")
            .with_tracestate(parse_tracestate(["vendor=value"]));
        assert_eq!(trace.tracestate(), Some("vendor=value"));
    }

    #[test]
    fn accepts_and_omits_empty_tracestate_list_members() {
        assert_eq!(
            parse_tracestate([" , vendor=value,,\t", "tenant@system=opaque,"]),
            Some("vendor=value,tenant@system=opaque".to_owned())
        );
        assert!(parse_tracestate([" , \t,"]).is_none());

        let thirty_two_with_empty_members = format!(
            ",{},,",
            (0..32)
                .map(|index| format!("k{index}=v"))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert!(parse_tracestate([thirty_two_with_empty_members.as_str()]).is_some());
    }

    #[test]
    fn rejects_duplicate_invalid_and_over_limit_tracestate() {
        assert!(parse_tracestate(std::iter::empty()).is_none());
        assert!(parse_tracestate(["a=1,a=2"]).is_none());
        assert!(parse_tracestate(["a=1", "a=2"]).is_none());
        assert!(parse_tracestate(["Upper=1"]).is_none());
        assert!(parse_tracestate(["a=control\u{7f}"]).is_none());
        assert!(parse_tracestate(["a=contains=value"]).is_none());
        let oversized = format!("a={}", "x".repeat(511));
        assert!(parse_tracestate([oversized.as_str()]).is_none());
    }

    #[test]
    fn enforces_tracestate_total_and_member_boundaries() {
        let maximum = format!("{}={}", "a".repeat(256), "v".repeat(255));
        assert_eq!(maximum.len(), 512);
        assert!(parse_tracestate([maximum.as_str()]).is_some());
        let oversized = format!("{}={}", "a".repeat(256), "v".repeat(256));
        assert_eq!(oversized.len(), 513);
        assert!(parse_tracestate([oversized.as_str()]).is_none());

        let thirty_two = (0..32)
            .map(|index| format!("k{index}=v"))
            .collect::<Vec<_>>()
            .join(",");
        assert!(parse_tracestate([thirty_two.as_str()]).is_some());
        let thirty_three = format!("{thirty_two},last=v");
        assert!(parse_tracestate([thirty_three.as_str()]).is_none());
    }

    #[test]
    fn enforces_simple_and_multi_tenant_key_grammar() {
        let simple_max = "a".repeat(256);
        let tenant_max = "1".repeat(241);
        let system_max = "s".repeat(14);
        for valid in [
            "a=1".to_owned(),
            "a0_-*/=1".to_owned(),
            format!("{simple_max}=1"),
            "1tenant@system=1".to_owned(),
            format!("{tenant_max}@{system_max}=1"),
        ] {
            assert!(
                parse_tracestate([valid.as_str()]).is_some(),
                "rejected {valid:?}"
            );
        }

        for invalid in [
            "1simple=1".to_owned(),
            "Upper=1".to_owned(),
            "a:bad=1".to_owned(),
            format!("{}=1", "a".repeat(257)),
            "@system=1".to_owned(),
            "Upper@system=1".to_owned(),
            "tenant@=1".to_owned(),
            "tenant@1system=1".to_owned(),
            "tenant@System=1".to_owned(),
            "tenant@sys:tem=1".to_owned(),
            format!("{}@system=1", "a".repeat(242)),
            format!("tenant@{}=1", "s".repeat(15)),
            "tenant@system@again=1".to_owned(),
        ] {
            assert!(
                parse_tracestate([invalid.as_str()]).is_none(),
                "accepted {invalid:?}"
            );
        }
    }

    #[test]
    fn enforces_tracestate_value_grammar() {
        let maximum = format!("a={}", "v".repeat(256));
        assert!(parse_tracestate([maximum.as_str()]).is_some());
        for invalid in [
            "a=".to_owned(),
            format!("a={}", "v".repeat(257)),
            "a=comma,value".to_owned(),
            "a=equals=value".to_owned(),
            "a=control\u{7f}".to_owned(),
        ] {
            assert!(
                parse_tracestate([invalid.as_str()]).is_none(),
                "accepted {invalid:?}"
            );
        }
    }

    fn encode_lower_hex(bytes: &[u8]) -> String {
        let mut encoded = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            write!(encoded, "{byte:02x}").expect("writing to a string is infallible");
        }
        encoded
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]

        #[test]
        fn round_trips_generated_valid_traceparents(
            trace_id in any::<[u8; 16]>()
                .prop_filter("trace ID must be nonzero", |value| value.iter().any(|byte| *byte != 0)),
            parent_id in any::<[u8; 8]>()
                .prop_filter("parent ID must be nonzero", |value| value.iter().any(|byte| *byte != 0)),
            flags in any::<u8>(),
        ) {
            let trace_id = encode_lower_hex(&trace_id);
            let parent_id = encode_lower_hex(&parent_id);
            let value = format!("00-{trace_id}-{parent_id}-{flags:02x}");

            let parsed = parse_traceparent(&value).expect("generated traceparent is valid");
            prop_assert_eq!(parsed.trace_id(), trace_id.as_str());
            prop_assert_eq!(parsed.parent_id(), parent_id.as_str());
            prop_assert_eq!(parsed.flags(), flags);
            prop_assert_eq!(parsed.sampled(), flags & 1 == 1);
            prop_assert_eq!(parsed.traceparent(), value.as_str());
        }

        #[test]
        fn preserves_generated_tracestate_across_header_splits(
            values in proptest::collection::vec("[A-Za-z0-9._~/-]{1,8}", 1..=32),
            split_mask in any::<u32>(),
        ) {
            let members = values
                .iter()
                .enumerate()
                .map(|(index, value)| format!("k{index}={value}"))
                .collect::<Vec<_>>();
            let combined = members.join(",");
            let mut fields = Vec::new();
            let mut field = String::new();

            for (index, member) in members.iter().enumerate() {
                if !field.is_empty() {
                    field.push(',');
                }
                field.push_str(member);
                if index + 1 < members.len() && split_mask & (1 << index) != 0 {
                    fields.push(std::mem::take(&mut field));
                }
            }
            fields.push(field);

            prop_assert_eq!(
                parse_tracestate(fields.iter().map(String::as_str)),
                Some(combined),
            );
        }
    }
}
