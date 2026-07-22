use std::collections::HashSet;

use crate::{TraceContext, TraceContextLevel};

/// Parses a single W3C `traceparent` value using strict lowercase framing.
#[cfg(test)]
#[must_use]
pub(crate) fn parse_traceparent(value: &str) -> Option<TraceContext> {
    parse_traceparent_with_level(value, TraceContextLevel::Level1)
}

/// Parses one W3C `traceparent` value for the selected Trace Context level.
#[must_use]
pub(crate) fn parse_traceparent_with_level(
    value: impl AsRef<[u8]>,
    level: TraceContextLevel,
) -> Option<TraceContext> {
    let bytes = value.as_ref();
    if bytes.len() < 55
        || bytes
            .iter()
            .any(|byte| (*byte < 0x20 && *byte != b'\t') || *byte == 0x7f)
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
    } else if bytes.len() > 55 && bytes[55] != b'-' {
        return None;
    }

    let version = decode_hex_byte(bytes[0], bytes[1]);
    let flags = decode_hex_byte(bytes[53], bytes[54]);
    Some(TraceContext::new(
        version,
        std::str::from_utf8(&bytes[3..35]).ok()?.to_owned(),
        std::str::from_utf8(&bytes[36..52]).ok()?.to_owned(),
        flags,
        level,
        bytes.to_vec().into_boxed_slice(),
    ))
}

/// Validates and combines W3C `tracestate` header values in wire order.
#[cfg(test)]
#[must_use]
pub(crate) fn parse_tracestate<'a>(values: impl IntoIterator<Item = &'a str>) -> Option<String> {
    parse_tracestate_with_level(values, TraceContextLevel::Level1)
}

/// Validates and combines W3C `tracestate` values for the selected level.
#[must_use]
pub(crate) fn parse_tracestate_with_level<'a>(
    values: impl IntoIterator<Item = &'a str>,
    level: TraceContextLevel,
) -> Option<String> {
    let values = values.into_iter().collect::<Vec<_>>();
    if values.is_empty() {
        return None;
    }
    let combined = values.join(",");
    let mut seen = HashSet::new();
    let members = combined
        .split(',')
        .map(|member| member.trim_matches([' ', '\t']))
        .collect::<Vec<_>>();
    if members.len() > 32 {
        return None;
    }

    for member in &members {
        if member.is_empty() {
            continue;
        }
        let (key, value) = member.split_once('=')?;
        if value.contains('=')
            || !is_valid_tracestate_key(key, level)
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

fn is_valid_tracestate_key(key: &str, level: TraceContextLevel) -> bool {
    if key.is_empty() || key.len() > 256 || !key.is_ascii() {
        return false;
    }

    match level {
        TraceContextLevel::Level1 => is_valid_level_one_key(key),
        TraceContextLevel::Level2 => {
            let first = key.as_bytes()[0];
            (first.is_ascii_lowercase() || first.is_ascii_digit())
                && key.bytes().all(is_level_two_key_char)
        }
    }
}

fn is_valid_level_one_key(key: &str) -> bool {
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

fn is_level_two_key_char(byte: u8) -> bool {
    is_key_char(byte) || byte == b'@'
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

    use super::{
        parse_traceparent, parse_traceparent_with_level, parse_tracestate,
        parse_tracestate_with_level,
    };
    use crate::TraceContextLevel;

    const VALID: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn parses_valid_context_and_sampled_bit() {
        let parsed = parse_traceparent(VALID).expect("valid traceparent");
        assert_eq!(parsed.trace_id(), "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(parsed.parent_id(), "00f067aa0ba902b7");
        assert_eq!(parsed.flags(), 1);
        assert!(parsed.sampled());
        assert_eq!(parsed.trace_context_level(), TraceContextLevel::Level1);
        assert_eq!(parsed.trace_id_random(), None);
        assert_eq!(parsed.traceparent(), Some(VALID));
        assert_eq!(parsed.traceparent_bytes(), VALID.as_bytes());
    }

    #[test]
    fn level_two_projects_the_random_trace_id_flag() {
        let random =
            parse_traceparent_with_level(VALID.replace("-01", "-03"), TraceContextLevel::Level2)
                .expect("flags 03");
        assert_eq!(random.trace_context_level(), TraceContextLevel::Level2);
        assert_eq!(random.trace_id_random(), Some(true));

        let not_random =
            parse_traceparent_with_level(VALID, TraceContextLevel::Level2).expect("flags 01");
        assert_eq!(not_random.trace_id_random(), Some(false));
    }

    #[test]
    fn future_version_level_two_preserves_sampling_without_assigning_random() {
        for (flags, sampled) in [("02", false), ("03", true)] {
            let value =
                format!("01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-{flags}-opaque");
            let trace = parse_traceparent_with_level(&value, TraceContextLevel::Level2)
                .expect("future version");
            assert_eq!(trace.sampled(), sampled);
            assert_eq!(trace.trace_id_random(), None);
            assert_eq!(trace.trace_context_level(), TraceContextLevel::Level2);
        }
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
    fn accepts_future_version_with_opaque_extensions() {
        let future = VALID.replacen("00-", "01-", 1);
        for value in [
            future.clone(),
            format!("{future}-"),
            format!("{future}-vendor"),
            format!("{future}-vendor space"),
            format!("{future}-~"),
        ] {
            assert!(parse_traceparent(&value).is_some(), "rejected {value:?}");
        }
    }

    #[test]
    fn enforces_native_field_safety_without_an_invented_length_ceiling() {
        let future = VALID.replacen("00-", "01-", 1);
        for invalid in [
            format!("{future}-opaque\u{1f}"),
            format!("{future}-opaque\u{7f}"),
        ] {
            assert!(
                parse_traceparent(&invalid).is_none(),
                "accepted {invalid:?}"
            );
        }
        let long = format!("{future}-{}", "x".repeat(512));
        assert!(long.len() > 512);
        assert!(parse_traceparent(&long).is_some());
        assert!(parse_traceparent(&format!("{future}-opaque-ümlaut")).is_some());

        let mut obs_text = format!("{future}-opaque-").into_bytes();
        obs_text.push(0x80);
        let parsed = parse_traceparent_with_level(&obs_text, TraceContextLevel::Level1)
            .expect("HTTP obs-text suffix remains opaque");
        assert_eq!(parsed.traceparent_bytes(), obs_text);
        assert_eq!(parsed.traceparent(), None);
    }

    #[test]
    fn rejects_invalid_future_delimiter() {
        let future = VALID.replacen("00-", "01-", 1);
        let invalid = format!("{future}vendor");
        assert!(
            parse_traceparent(&invalid).is_none(),
            "accepted {invalid:?}"
        );
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
    fn accepts_and_preserves_empty_tracestate_list_members() {
        assert_eq!(
            parse_tracestate([" , vendor=value,,\t", "tenant@system=opaque,"]),
            Some(",vendor=value,,,tenant@system=opaque,".to_owned())
        );
        assert_eq!(parse_tracestate([" , \t,"]), Some(",,".to_owned()));
        assert_eq!(parse_tracestate([""]), Some(String::new()));

        let thirty_two_with_empty_members = format!(
            ",{},",
            (0..30)
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
        let beyond_minimum = format!("{}={}", "a".repeat(256), "v".repeat(256));
        assert_eq!(beyond_minimum.len(), 513);
        assert!(parse_tracestate([beyond_minimum.as_str()]).is_some());

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
    fn level_two_uses_its_flat_at_sign_key_grammar() {
        for valid in ["1simple=1", "tenant@system@edge=1"] {
            assert!(
                parse_tracestate_with_level([valid], TraceContextLevel::Level2).is_some(),
                "rejected {valid:?}"
            );
            assert!(
                parse_tracestate_with_level([valid], TraceContextLevel::Level1).is_none(),
                "Level 1 accepted {valid:?}"
            );
        }

        for invalid in ["Upper=1", "tenant:@system=1", "@system=1"] {
            assert!(
                parse_tracestate_with_level([invalid], TraceContextLevel::Level2).is_none(),
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
            prop_assert_eq!(parsed.traceparent(), Some(value.as_str()));
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
