use super::*;

fn array(member: &Type, dimensions: &[(i32, i32)], values: &[Option<&[u8]>]) -> Vec<u8> {
    let mut raw = Vec::new();
    raw.extend_from_slice(&(dimensions.len() as i32).to_be_bytes());
    raw.extend_from_slice(&1_i32.to_be_bytes());
    raw.extend_from_slice(&member.oid().to_be_bytes());
    for (len, lower) in dimensions {
        raw.extend_from_slice(&len.to_be_bytes());
        raw.extend_from_slice(&lower.to_be_bytes());
    }
    for value in values {
        raw.extend_from_slice(
            &value
                .map(|value| value.len() as i32)
                .unwrap_or(-1)
                .to_be_bytes(),
        );
        if let Some(value) = value {
            raw.extend_from_slice(value);
        }
    }
    raw
}

#[test]
fn arrays_preserve_dimensions_bounds_empty_strings_and_nulls() {
    let raw = array(
        &Type::TEXT,
        &[(2, -1), (2, 4)],
        &[Some(b""), None, Some(b"NULL"), Some(b"null")],
    );
    assert_eq!(
        format_raw(&Type::TEXT_ARRAY, &raw),
        "[-1:0][4:5]={{\"\", NULL}, {\"NULL\", \"null\"}}"
    );
    assert_eq!(
        format_raw(&Type::TEXT_ARRAY, &array(&Type::TEXT, &[], &[])),
        "{}"
    );
    let raw = array(
        &Type::TEXT,
        &[(2, 1)],
        &[Some(b"quote\"slash\\"), Some(b"a\nb")],
    );
    assert_eq!(
        format_raw(&Type::TEXT_ARRAY, &raw),
        "{\"quote\\\"slash\\\\\", \"a\nb\"}"
    );
}

#[test]
fn hstore_uses_driver_decoding_and_deterministic_escaping() {
    let type_ = Type::new("hstore".into(), 50000, Kind::Simple, "extensions".into());
    let mut raw = 3_i32.to_be_bytes().to_vec();
    for (key, value) in [("z", Some("NULL")), ("a\"", None), ("b", Some("žluť\\"))] {
        raw.extend_from_slice(&(key.len() as i32).to_be_bytes());
        raw.extend_from_slice(key.as_bytes());
        raw.extend_from_slice(
            &value
                .map(|value| value.len() as i32)
                .unwrap_or(-1)
                .to_be_bytes(),
        );
        if let Some(value) = value {
            raw.extend_from_slice(value.as_bytes());
        }
    }
    assert_eq!(
        format_raw(&type_, &raw),
        "\"a\\\"\"=>NULL, \"b\"=>\"žluť\\\\\", \"z\"=>\"NULL\""
    );
    assert_eq!(format_raw(&type_, &0_i32.to_be_bytes()), "");
    let domain = Type::new(
        "attributes".into(),
        50001,
        Kind::Domain(type_),
        "public".into(),
    );
    assert!(format_raw(&domain, &raw).starts_with("\"a"));
}

#[test]
fn money_uses_explicit_scale_without_float_rounding_or_overflow() {
    for (value, scale, expected) in [
        (12345_i64, 2, "123.45"),
        (-1, 3, "-0.001"),
        (123, 0, "123"),
        (i64::MIN, 2, "-92233720368547758.08"),
    ] {
        assert_eq!(
            wire::money(&value.to_be_bytes(), scale).as_deref(),
            Some(expected)
        );
    }
    assert_eq!(wire::money(&[0; 8], 11), None);
}

#[test]
fn ranges_and_multiranges_preserve_empty_and_unbounded_values() {
    assert_eq!(format_raw(&Type::INT4_RANGE, &[1]), "empty");
    assert_eq!(format_raw(&Type::INT4_RANGE, &[0x18]), "(,)");
    let mut range = vec![2];
    for value in [1_i32, 5] {
        range.extend_from_slice(&4_i32.to_be_bytes());
        range.extend_from_slice(&value.to_be_bytes());
    }
    assert_eq!(format_raw(&Type::INT4_RANGE, &range), "[1,5)");
    let mut multi = 1_i32.to_be_bytes().to_vec();
    multi.extend_from_slice(&(range.len() as i32).to_be_bytes());
    multi.extend_from_slice(&range);
    assert_eq!(format_raw(&Type::INT4MULTI_RANGE, &multi), "{[1,5)}");
}

#[test]
fn malformed_container_payloads_fall_back_without_panicking() {
    let valid = array(&Type::TEXT, &[(1, 1)], &[Some(b"abc")]);
    for len in 0..valid.len() {
        assert!(containers::array(&Type::TEXT, &valid[..len], ValueFormat::default()).is_none());
    }
    let mut oversized = valid.clone();
    oversized[12..16].copy_from_slice(&i32::MAX.to_be_bytes());
    assert!(containers::array(&Type::TEXT, &oversized, ValueFormat::default()).is_none());
    let mut trailing = valid;
    trailing.push(0);
    assert!(containers::array(&Type::TEXT, &trailing, ValueFormat::default()).is_none());
    assert!(text_search::query(&[0, 0, 0, 1, 2, 255]).is_none());
    assert!(geometry::format(&Type::PATH, &[1, 255, 255, 255, 255]).is_none());
    assert!(wire::snapshot(&[0; 19]).is_none());
}

#[test]
fn jsonpath_and_snapshot_have_readable_representations() {
    assert_eq!(format_raw(&Type::JSONPATH, b"\x01$.items[*]"), "$.items[*]");
    let mut raw = 2_i32.to_be_bytes().to_vec();
    for value in [10_u64, 20, 11, 18] {
        raw.extend_from_slice(&value.to_be_bytes());
    }
    assert_eq!(format_raw(&Type::PG_SNAPSHOT, &raw), "10:20:11,18");
}

#[test]
fn malformed_values_cannot_allocate_from_untrusted_counts_or_recurse_forever() {
    let hstore = Type::new("hstore".into(), 50000, Kind::Simple, "public".into());
    assert!(containers::hstore(&hstore, &i32::MAX.to_be_bytes()).is_none());
    let empty_dimensions = array(&Type::INT4, &[(i32::MAX, 1), (0, 1)], &[]);
    assert!(containers::array(&Type::INT4, &empty_dimensions, ValueFormat::default()).is_none());
    let mut nested = Type::INT4;
    for index in 0..100 {
        nested = Type::new(
            format!("domain_{index}"),
            60000 + index,
            Kind::Domain(nested),
            "public".into(),
        );
    }
    assert!(format_raw(&nested, &1_i32.to_be_bytes()).contains("binary bytes"));
    for type_ in [
        &hstore,
        &Type::INT4_ARRAY,
        &Type::INT4_RANGE,
        &Type::INT4MULTI_RANGE,
        &Type::RECORD,
        &Type::TS_VECTOR,
        &Type::TSQUERY,
        &Type::PATH,
        &Type::POLYGON,
        &Type::PG_SNAPSHOT,
    ] {
        let mut state = 17_u32;
        for length in 0..128 {
            let raw = (0..length)
                .map(|_| {
                    state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                    (state >> 24) as u8
                })
                .collect::<Vec<_>>();
            let _ = format_raw(type_, &raw);
        }
    }
}
