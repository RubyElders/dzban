use super::{ValueFormat, format_with, wire::Reader};
use fallible_iterator::FallibleIterator;
use postgres_protocol::types::{self, Range, RangeBound};
use std::collections::HashMap;
use tokio_postgres::types::{FromSql, Kind, Type};

fn quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

pub(super) fn hstore(type_: &Type, raw: &[u8]) -> Option<String> {
    Reader(raw).count(8)?;
    let values = <HashMap<String, Option<String>> as FromSql>::from_sql(type_, raw).ok()?;
    let mut pairs = values.into_iter().collect::<Vec<_>>();
    pairs.sort_unstable_by(|a, b| a.0.cmp(&b.0));
    Some(
        pairs
            .into_iter()
            .map(|(key, value)| {
                format!(
                    "{}=>{}",
                    quoted(&key),
                    value
                        .as_deref()
                        .map(quoted)
                        .unwrap_or_else(|| "NULL".into())
                )
            })
            .collect::<Vec<_>>()
            .join(", "),
    )
}

pub(super) fn array(member: &Type, raw: &[u8], format: ValueFormat) -> Option<String> {
    validate_array(raw)?;
    let array = types::array_from_sql(raw).ok()?;
    let dimensions = array.dimensions().collect::<Vec<_>>().ok()?;
    if dimensions.len() > 6 {
        return None;
    }
    let values = array.values().collect::<Vec<_>>().ok()?;
    let mut base = member;
    while let Kind::Domain(inner) = base.kind() {
        base = inner;
    }
    let delimiter = if *base == Type::BOX { ';' } else { ',' };
    let mut values = values.into_iter();
    fn level<'a>(
        dimensions: &[types::ArrayDimension],
        values: &mut impl Iterator<Item = Option<&'a [u8]>>,
        member: &Type,
        delimiter: char,
        format: ValueFormat,
    ) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(dimension) = dimensions.first() {
            for _ in 0..dimension.len {
                parts.push(if dimensions.len() > 1 {
                    level(&dimensions[1..], values, member, delimiter, format)?
                } else {
                    match values.next()? {
                        None => "NULL".into(),
                        Some(raw) => {
                            let value = format_with(member, raw, format);
                            if value.is_empty()
                                || value.eq_ignore_ascii_case("NULL")
                                || value.chars().any(|c| {
                                    c.is_whitespace()
                                        || [delimiter, '{', '}', '"', '\\'].contains(&c)
                                })
                            {
                                quoted(&value)
                            } else {
                                value
                            }
                        }
                    }
                });
            }
        }
        Some(format!("{{{}}}", parts.join(&format!("{delimiter} "))))
    }
    let value = level(&dimensions, &mut values, member, delimiter, format)?;
    let mut bounds = String::new();
    if dimensions
        .iter()
        .any(|dimension| dimension.lower_bound != 1)
    {
        for dimension in dimensions {
            bounds.push_str(&format!(
                "[{}:{}]",
                dimension.lower_bound,
                i64::from(dimension.lower_bound) + i64::from(dimension.len) - 1
            ));
        }
        bounds.push('=');
    }
    Some(format!("{bounds}{value}"))
}

pub(super) fn vector(type_: &Type, raw: &[u8], format: ValueFormat) -> Option<String> {
    validate_array(raw)?;
    let member = if *type_ == Type::INT2_VECTOR {
        Type::INT2
    } else {
        Type::OID
    };
    let array = types::array_from_sql(raw).ok()?;
    let values = array.values().collect::<Vec<_>>().ok()?;
    let parts = values
        .into_iter()
        .map(|value| Some(format_with(&member, value?, format)))
        .collect::<Option<Vec<_>>>()?;
    Some(parts.join(" "))
}

fn validate_array(raw: &[u8]) -> Option<()> {
    let mut reader = Reader(raw);
    let dimensions = usize::try_from(reader.i32()?).ok()?;
    if dimensions > 6 {
        return None;
    }
    reader.u32()?;
    reader.u32()?;
    let mut elements = usize::from(dimensions != 0);
    for _ in 0..dimensions {
        let length = usize::try_from(reader.i32()?).ok()?;
        if length == 0 {
            return None;
        }
        elements = elements.checked_mul(length)?;
        reader.i32()?;
    }
    (elements <= reader.0.len() / 4).then_some(())
}

pub(super) fn range(member: &Type, raw: &[u8], format: ValueFormat) -> Option<String> {
    if raw.first().is_none_or(|flags| *flags & !0x1f != 0) {
        return None;
    }
    match types::range_from_sql(raw).ok()? {
        Range::Empty => Some("empty".into()),
        Range::Nonempty(lower, upper) => {
            fn bound(
                bound: RangeBound<Option<&[u8]>>,
                member: &Type,
                format: ValueFormat,
            ) -> Option<(bool, String)> {
                let (inclusive, raw) = match bound {
                    RangeBound::Unbounded => return Some((false, String::new())),
                    RangeBound::Inclusive(raw) => (true, raw?),
                    RangeBound::Exclusive(raw) => (false, raw?),
                };
                let value = format_with(member, raw, format);
                let value = if value.is_empty()
                    || value.chars().any(|c| {
                        c.is_whitespace() || [',', '(', ')', '[', ']', '"', '\\'].contains(&c)
                    }) {
                    quoted(&value)
                } else {
                    value
                };
                Some((inclusive, value))
            }
            let (left, lower) = bound(lower, member, format)?;
            let (right, upper) = bound(upper, member, format)?;
            Some(format!(
                "{}{lower},{upper}{}",
                if left { '[' } else { '(' },
                if right { ']' } else { ')' }
            ))
        }
    }
}

pub(super) fn multirange(member: &Type, raw: &[u8], format: ValueFormat) -> Option<String> {
    let mut reader = Reader(raw);
    let count = reader.count(5)?;
    let mut parts = Vec::new();
    for _ in 0..count {
        parts.push(range(member, reader.value()??, format)?);
    }
    reader.finish(format!("{{{}}}", parts.join(",")))
}

pub(super) fn record(type_: &Type, raw: &[u8], format: ValueFormat) -> Option<String> {
    let mut reader = Reader(raw);
    let count = reader.count(8)?;
    let fields = match type_.kind() {
        Kind::Composite(fields) if fields.len() == count => Some(fields),
        Kind::Composite(_) => return None,
        _ => None,
    };
    let mut parts = Vec::new();
    for index in 0..count {
        let oid = reader.u32()?;
        let type_ = fields
            .and_then(|fields| fields.get(index))
            .map(|field| field.type_().clone())
            .or_else(|| Type::from_oid(oid))?;
        if type_.oid() != oid {
            return None;
        }
        parts.push(match reader.value()? {
            None => String::new(),
            Some(raw) => {
                let value = format_with(&type_, raw, format);
                if value.is_empty()
                    || value
                        .chars()
                        .any(|c| c.is_whitespace() || [',', '(', ')', '"', '\\'].contains(&c))
                {
                    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\"\""))
                } else {
                    value
                }
            }
        });
    }
    reader.finish(format!("({})", parts.join(",")))
}
