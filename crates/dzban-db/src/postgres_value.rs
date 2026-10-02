use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use tokio_postgres::Row;
use tokio_postgres::types::{FromSql, Kind, Type};

struct RawValue(Vec<u8>);

impl<'a> FromSql<'a> for RawValue {
    fn from_sql(_: &Type, raw: &'a [u8]) -> Result<Self, Box<dyn Error + Sync + Send>> {
        Ok(Self(raw.to_vec()))
    }

    fn accepts(_: &Type) -> bool {
        true
    }
}

pub(crate) fn display_value(row: &Row, index: usize) -> String {
    if let Ok(value) = row.try_get::<_, Option<String>>(index) {
        return value.unwrap_or_else(|| "NULL".to_string());
    }
    if let Ok(value) = row.try_get::<_, Option<i8>>(index) {
        return value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }
    if let Ok(value) = row.try_get::<_, Option<i16>>(index) {
        return value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }
    if let Ok(value) = row.try_get::<_, Option<i32>>(index) {
        return value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }
    if let Ok(value) = row.try_get::<_, Option<i64>>(index) {
        return value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }
    if let Ok(value) = row.try_get::<_, Option<u32>>(index) {
        return value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }
    if let Ok(value) = row.try_get::<_, Option<f32>>(index) {
        return value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }
    if let Ok(value) = row.try_get::<_, Option<f64>>(index) {
        return value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }
    if let Ok(value) = row.try_get::<_, Option<bool>>(index) {
        return value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }
    if let Ok(value) = row.try_get::<_, Option<serde_json::Value>>(index) {
        return value
            .map(|value| value.to_string())
            .unwrap_or_else(|| "NULL".to_string());
    }

    let type_ = row.columns()[index].type_();
    match row.try_get::<_, Option<RawValue>>(index) {
        Ok(Some(value)) => format_raw(type_, &value.0),
        Ok(None) => "NULL".to_string(),
        Err(_) => format!("Unable to display {} value", type_.name()),
    }
}

pub(crate) fn is_null(row: &Row, index: usize) -> bool {
    matches!(row.try_get::<_, Option<RawValue>>(index), Ok(None))
}

fn format_raw(type_: &Type, raw: &[u8]) -> String {
    if let Kind::Domain(inner) = type_.kind() {
        return format_raw(inner, raw);
    }
    if let Kind::Enum(_) = type_.kind() {
        return String::from_utf8_lossy(raw).into_owned();
    }
    if let Kind::Array(member) = type_.kind()
        && let Ok(values) = <Vec<Option<RawValue>> as FromSql>::from_sql(type_, raw)
    {
        let values = values
            .into_iter()
            .map(|value| {
                value
                    .map(|value| format_array_value(member, &value.0))
                    .unwrap_or_else(|| "NULL".to_string())
            })
            .collect::<Vec<_>>();
        return format!("{{{}}}", values.join(", "));
    }

    match type_.name() {
        "bool" if raw.len() == 1 => match raw[0] {
            0 => "false".to_string(),
            1 => "true".to_string(),
            _ => fallback(type_, raw),
        },
        "char" if raw.len() == 1 => (raw[0] as i8).to_string(),
        "int2" => read_i16(raw)
            .map(|value| value.to_string())
            .unwrap_or_else(|| fallback(type_, raw)),
        "int4" => read_i32(raw)
            .map(|value| value.to_string())
            .unwrap_or_else(|| fallback(type_, raw)),
        "int8" => read_i64(raw)
            .map(|value| value.to_string())
            .unwrap_or_else(|| fallback(type_, raw)),
        "float4" => read_u32(raw)
            .map(f32::from_bits)
            .map(|value| value.to_string())
            .unwrap_or_else(|| fallback(type_, raw)),
        "float8" => read_u64(raw)
            .map(f64::from_bits)
            .map(|value| value.to_string())
            .unwrap_or_else(|| fallback(type_, raw)),
        "text" | "varchar" | "bpchar" | "name" | "unknown" | "xml" | "citext" | "ltree"
        | "lquery" | "ltxtquery" => String::from_utf8_lossy(raw).into_owned(),
        "json" => String::from_utf8_lossy(raw).into_owned(),
        "jsonb" if raw.first() == Some(&1) => String::from_utf8_lossy(&raw[1..]).into_owned(),
        "xid" | "cid" | "oid" | "regproc" | "regprocedure" | "regoper" | "regoperator"
        | "regclass" | "regtype" | "regconfig" | "regdictionary" | "regnamespace" | "regrole"
        | "regcollation" => read_u32(raw)
            .map(|value| value.to_string())
            .unwrap_or_else(|| fallback(type_, raw)),
        "xid8" => read_u64(raw)
            .map(|value| value.to_string())
            .unwrap_or_else(|| fallback(type_, raw)),
        "tid" if raw.len() == 6 => format!(
            "({},{})",
            u32::from_be_bytes(raw[0..4].try_into().unwrap()),
            u16::from_be_bytes(raw[4..6].try_into().unwrap())
        ),
        "uuid" if raw.len() == 16 => format_uuid(raw),
        "bytea" => format!("\\x{}", hex(raw)),
        "numeric" => format_numeric(raw).unwrap_or_else(|| fallback(type_, raw)),
        "date" => format_date_value(raw).unwrap_or_else(|| fallback(type_, raw)),
        "time" => read_i64(raw)
            .map(format_time)
            .unwrap_or_else(|| fallback(type_, raw)),
        "timetz" => format_timetz(raw).unwrap_or_else(|| fallback(type_, raw)),
        "timestamp" => format_timestamp_value(raw, false).unwrap_or_else(|| fallback(type_, raw)),
        "timestamptz" => format_timestamp_value(raw, true).unwrap_or_else(|| fallback(type_, raw)),
        "interval" => format_interval(raw).unwrap_or_else(|| fallback(type_, raw)),
        "inet" | "cidr" => format_inet(raw).unwrap_or_else(|| fallback(type_, raw)),
        "macaddr" if raw.len() == 6 => raw
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(":"),
        "macaddr8" if raw.len() == 8 => raw
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(":"),
        "pg_lsn" => read_u64(raw)
            .map(|value| format!("{:X}/{:X}", value >> 32, value & 0xffff_ffff))
            .unwrap_or_else(|| fallback(type_, raw)),
        "bit" | "varbit" => format_bits(raw).unwrap_or_else(|| fallback(type_, raw)),
        _ => fallback(type_, raw),
    }
}

fn format_array_value(type_: &Type, raw: &[u8]) -> String {
    let value = format_raw(type_, raw);
    if value.contains([',', '{', '}', '"', '\\', ' ', '\n', '\r', '\t']) {
        format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        value
    }
}

fn fallback(type_: &Type, raw: &[u8]) -> String {
    std::str::from_utf8(raw)
        .ok()
        .filter(|value| !value.chars().any(char::is_control))
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!(
                "{} ({} binary bytes: 0x{})",
                type_.name(),
                raw.len(),
                hex(raw)
            )
        })
}

fn read_i16(raw: &[u8]) -> Option<i16> {
    Some(i16::from_be_bytes(raw.try_into().ok()?))
}

fn read_u16(raw: &[u8]) -> Option<u16> {
    Some(u16::from_be_bytes(raw.try_into().ok()?))
}

fn read_i32(raw: &[u8]) -> Option<i32> {
    Some(i32::from_be_bytes(raw.try_into().ok()?))
}

fn read_u32(raw: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(raw.try_into().ok()?))
}

fn read_i64(raw: &[u8]) -> Option<i64> {
    Some(i64::from_be_bytes(raw.try_into().ok()?))
}

fn read_u64(raw: &[u8]) -> Option<u64> {
    Some(u64::from_be_bytes(raw.try_into().ok()?))
}

fn hex(raw: &[u8]) -> String {
    raw.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn format_uuid(raw: &[u8]) -> String {
    let value = hex(raw);
    format!(
        "{}-{}-{}-{}-{}",
        &value[0..8],
        &value[8..12],
        &value[12..16],
        &value[16..20],
        &value[20..32]
    )
}

fn format_numeric(raw: &[u8]) -> Option<String> {
    if raw.len() < 8 || !(raw.len() - 8).is_multiple_of(2) {
        return None;
    }
    let digit_count = read_i16(&raw[0..2])? as usize;
    let weight = read_i16(&raw[2..4])? as isize;
    let sign = read_u16(&raw[4..6])?;
    let scale = read_u16(&raw[6..8])? as usize;
    if digit_count * 2 + 8 != raw.len() {
        return None;
    }
    match sign {
        0xc000 => return Some("NaN".to_string()),
        0xd000 => return Some("Infinity".to_string()),
        0xf000 => return Some("-Infinity".to_string()),
        0x0000 | 0x4000 => {}
        _ => return None,
    }
    let mut digits = Vec::with_capacity(digit_count);
    for index in 0..digit_count {
        digits.push(read_u16(&raw[8 + index * 2..10 + index * 2])?);
    }
    if digits.iter().any(|digit| *digit > 9999) {
        return None;
    }

    let mut integer = String::new();
    if weight < 0 {
        integer.push('0');
    } else {
        for position in (0..=weight).rev() {
            let index = weight - position;
            let digit = digits.get(index as usize).copied().unwrap_or(0);
            if integer.is_empty() {
                integer.push_str(&digit.to_string());
            } else {
                integer.push_str(&format!("{digit:04}"));
            }
        }
    }

    let mut fraction = String::new();
    for position in 1..=scale.div_ceil(4) {
        let index = weight + position as isize;
        let digit = if index >= 0 {
            digits.get(index as usize).copied().unwrap_or(0)
        } else {
            0
        };
        fraction.push_str(&format!("{digit:04}"));
    }
    fraction.truncate(scale);

    let negative = sign == 0x4000 && digits.iter().any(|digit| *digit != 0);
    Some(format!(
        "{}{}{}",
        if negative { "-" } else { "" },
        integer,
        if scale == 0 {
            String::new()
        } else {
            format!(".{fraction}")
        }
    ))
}

fn format_date_value(raw: &[u8]) -> Option<String> {
    match read_i32(raw)? {
        i32::MAX => Some("infinity".to_string()),
        i32::MIN => Some("-infinity".to_string()),
        days => Some(format_date(days as i64 + 10_957)),
    }
}

fn format_timestamp_value(raw: &[u8], utc: bool) -> Option<String> {
    match read_i64(raw)? {
        i64::MAX => Some("infinity".to_string()),
        i64::MIN => Some("-infinity".to_string()),
        micros => {
            let day_micros = 86_400_000_000_i64;
            let days = micros.div_euclid(day_micros);
            let time = micros.rem_euclid(day_micros);
            Some(format!(
                "{} {}{}",
                format_date(days + 10_957),
                format_time(time),
                if utc { "+00" } else { "" }
            ))
        }
    }
}

fn format_date(days_since_unix_epoch: i64) -> String {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

fn format_time(micros: i64) -> String {
    let hours = micros / 3_600_000_000;
    let minutes = micros / 60_000_000 % 60;
    let seconds = micros / 1_000_000 % 60;
    let remainder = micros % 1_000_000;
    if remainder == 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{hours:02}:{minutes:02}:{seconds:02}.{remainder:06}")
            .trim_end_matches('0')
            .to_string()
    }
}

fn format_timetz(raw: &[u8]) -> Option<String> {
    if raw.len() != 12 {
        return None;
    }
    let time = read_i64(&raw[0..8])?;
    let offset = -read_i32(&raw[8..12])?;
    let sign = if offset < 0 { '-' } else { '+' };
    let offset = offset.unsigned_abs();
    Some(format!(
        "{}{}{:02}:{:02}",
        format_time(time),
        sign,
        offset / 3600,
        offset / 60 % 60
    ))
}

fn format_interval(raw: &[u8]) -> Option<String> {
    if raw.len() != 16 {
        return None;
    }
    let micros = read_i64(&raw[0..8])?;
    let days = read_i32(&raw[8..12])?;
    let months = read_i32(&raw[12..16])?;
    Some(format!("{months} mons {days} days {}", format_time(micros)))
}

fn format_inet(raw: &[u8]) -> Option<String> {
    if raw.len() < 4 {
        return None;
    }
    let netmask = raw[1];
    let length = raw[3] as usize;
    if raw.len() != length + 4 {
        return None;
    }
    let address = match (raw[0], length) {
        (2, 4) => IpAddr::V4(Ipv4Addr::new(raw[4], raw[5], raw[6], raw[7])),
        (3, 16) => IpAddr::V6(Ipv6Addr::from(<[u8; 16]>::try_from(&raw[4..]).ok()?)),
        _ => return None,
    };
    let full_mask = if address.is_ipv4() { 32 } else { 128 };
    if netmask == full_mask {
        Some(address.to_string())
    } else {
        Some(format!("{address}/{netmask}"))
    }
}

fn format_bits(raw: &[u8]) -> Option<String> {
    if raw.len() < 4 {
        return None;
    }
    let bit_count = read_i32(&raw[0..4])?;
    if bit_count < 0 || raw.len() != 4 + (bit_count as usize).div_ceil(8) {
        return None;
    }
    let mut result = String::with_capacity(bit_count as usize);
    for bit in 0..bit_count as usize {
        let byte = raw[4 + bit / 8];
        result.push(if byte & (1 << (7 - bit % 8)) == 0 {
            '0'
        } else {
            '1'
        });
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_postgres_system_identifiers() {
        assert_eq!(format_raw(&Type::OID, &42_u32.to_be_bytes()), "42");
        assert_eq!(format_raw(&Type::XID, &123_u32.to_be_bytes()), "123");
        assert_eq!(
            format_raw(&Type::XID8, &u64::MAX.to_be_bytes()),
            u64::MAX.to_string()
        );
        assert_eq!(format_raw(&Type::TID, &[0, 0, 0, 7, 0, 3]), "(7,3)");
    }

    #[test]
    fn formats_numeric_scale_and_sign() {
        assert_eq!(
            format_numeric(&[0, 2, 0, 0, 0, 0, 0, 2, 0, 12, 19, 136]).as_deref(),
            Some("12.50")
        );
        assert_eq!(
            format_numeric(&[0, 1, 0, 0, 0x40, 0, 0, 0, 0, 7]).as_deref(),
            Some("-7")
        );
    }

    #[test]
    fn formats_dates_times_and_networks() {
        assert_eq!(
            format_date_value(&0_i32.to_be_bytes()).as_deref(),
            Some("2000-01-01")
        );
        assert_eq!(format_time(45_296_123_000), "12:34:56.123");
        assert_eq!(
            format_inet(&[2, 24, 0, 4, 192, 168, 1, 4]).as_deref(),
            Some("192.168.1.4/24")
        );
    }

    #[test]
    fn unknown_values_show_their_data_instead_of_a_type_placeholder() {
        let type_ = Type::new(
            "extension_type".to_string(),
            42_000,
            Kind::Simple,
            "public".to_string(),
        );
        assert_eq!(fallback(&type_, b"readable"), "readable");
        assert_eq!(
            fallback(&type_, &[0xff, 0]),
            "extension_type (2 binary bytes: 0xff00)"
        );
    }

    #[tokio::test]
    #[ignore = "requires test PostgreSQL"]
    async fn formats_values_from_postgres_rows() {
        let (client, connection) = tokio_postgres::connect(
            "host=localhost port=5415 user=testuser password=testpass dbname=testdb",
            tokio_postgres::NoTls,
        )
        .await
        .unwrap();
        tokio::spawn(async move { connection.await.unwrap() });
        let row = client
            .query_one(
                "SELECT 42::oid, '123'::xid, '456'::xid8, '(7,3)'::tid, \
                 12.50::numeric, DATE '2026-09-23', \
                 '550e8400-e29b-41d4-a716-446655440000'::uuid, \
                 '192.168.1.4/24'::inet, ARRAY[1, 2, 3]::int4[]",
                &[],
            )
            .await
            .unwrap();

        assert_eq!(display_value(&row, 0), "42");
        assert_eq!(display_value(&row, 1), "123");
        assert_eq!(display_value(&row, 2), "456");
        assert_eq!(display_value(&row, 3), "(7,3)");
        assert_eq!(display_value(&row, 4), "12.50");
        assert_eq!(display_value(&row, 5), "2026-09-23");
        assert_eq!(
            display_value(&row, 6),
            "550e8400-e29b-41d4-a716-446655440000"
        );
        assert_eq!(display_value(&row, 7), "192.168.1.4/24");
        assert_eq!(display_value(&row, 8), "{1, 2, 3}");
    }
}
