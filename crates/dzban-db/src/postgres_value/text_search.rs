use super::wire::Reader;

fn lexeme(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "''"))
}

pub(super) fn vector(raw: &[u8]) -> Option<String> {
    let mut reader = Reader(raw);
    let count = reader.count(4)?;
    let mut words = Vec::new();
    for _ in 0..count {
        let mut word = lexeme(reader.string()?);
        let positions = reader.u16()?;
        let mut values = Vec::new();
        for _ in 0..positions {
            let value = reader.u16()?;
            let weight = match value >> 14 {
                0 => "",
                1 => "C",
                2 => "B",
                3 => "A",
                _ => return None,
            };
            values.push(format!("{}{weight}", value & 0x3fff));
        }
        if positions > 0 {
            word.push(':');
            word.push_str(&values.join(","));
        }
        words.push(word);
    }
    reader.finish(words.join(" "))
}

pub(super) fn query(raw: &[u8]) -> Option<String> {
    let mut reader = Reader(raw);
    let mut count = reader.count(2)?;
    if count == 0 {
        return reader.finish(String::new());
    }
    fn node(reader: &mut Reader<'_>, count: &mut usize, depth: usize) -> Option<String> {
        if depth > 64 || *count == 0 {
            return None;
        }
        *count -= 1;
        match reader.byte()? {
            1 => {
                let weight = reader.byte()?;
                let prefix = reader.byte()?;
                if weight > 15 || prefix > 1 {
                    return None;
                }
                let mut word = lexeme(reader.string()?);
                if weight != 0 || prefix != 0 {
                    word.push(':');
                    if prefix != 0 {
                        word.push('*');
                    }
                    for (bit, letter) in [(8, 'A'), (4, 'B'), (2, 'C'), (1, 'D')] {
                        if weight & bit != 0 {
                            word.push(letter);
                        }
                    }
                }
                Some(word)
            }
            2 => {
                let operator = reader.byte()?;
                if operator == 1 {
                    return Some(format!("!({})", node(reader, count, depth + 1)?));
                }
                let operator = match operator {
                    2 => "&".into(),
                    3 => "|".into(),
                    4 => match reader.u16()? {
                        1 => "<->".into(),
                        distance => format!("<{distance}>"),
                    },
                    _ => return None,
                };
                let right = node(reader, count, depth + 1)?;
                let left = node(reader, count, depth + 1)?;
                Some(format!("({left} {operator} {right})"))
            }
            _ => None,
        }
    }
    let value = node(&mut reader, &mut count, 0)?;
    if count != 0 {
        return None;
    }
    reader.finish(value)
}
