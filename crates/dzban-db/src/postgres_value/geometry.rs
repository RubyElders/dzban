use super::wire::Reader;
use fallible_iterator::FallibleIterator;
use postgres_protocol::types;
use tokio_postgres::types::Type;

fn number(value: f64) -> String {
    if value.is_infinite() {
        if value.is_sign_positive() {
            "Infinity".into()
        } else {
            "-Infinity".into()
        }
    } else {
        value.to_string()
    }
}

fn point(value: types::Point) -> String {
    format!("({},{})", number(value.x()), number(value.y()))
}

pub(super) fn format(type_: &Type, raw: &[u8]) -> Option<String> {
    match type_.name() {
        "point" => Some(point(types::point_from_sql(raw).ok()?)),
        "box" => {
            let value = types::box_from_sql(raw).ok()?;
            Some(format!(
                "{},{}",
                point(value.upper_right()),
                point(value.lower_left())
            ))
        }
        "path" => {
            if !matches!(raw.first(), Some(0 | 1)) {
                return None;
            }
            let mut reader = Reader(&raw[1..]);
            let count = reader.count(16)?;
            if reader.0.len() != count * 16 {
                return None;
            }
            let value = types::path_from_sql(raw).ok()?;
            let points = value
                .points()
                .map(|value| Ok(point(value)))
                .collect::<Vec<_>>()
                .ok()?;
            Some(format!(
                "{}{}{}",
                if value.closed() { '(' } else { '[' },
                points.join(","),
                if value.closed() { ')' } else { ']' }
            ))
        }
        "line" | "circle" | "lseg" => {
            let mut reader = Reader(raw);
            let first = number(reader.f64()?);
            let second = number(reader.f64()?);
            let third = number(reader.f64()?);
            let value = match type_.name() {
                "line" => format!("{{{first},{second},{third}}}"),
                "circle" => format!("<({first},{second}),{third}>"),
                _ => format!("[({first},{second}),({third},{})]", number(reader.f64()?)),
            };
            reader.finish(value)
        }
        "polygon" => {
            let mut reader = Reader(raw);
            let count = reader.count(16)?;
            let mut points = Vec::new();
            for _ in 0..count {
                points.push(format!(
                    "({},{})",
                    number(reader.f64()?),
                    number(reader.f64()?)
                ));
            }
            reader.finish(format!("({})", points.join(",")))
        }
        _ => None,
    }
}
