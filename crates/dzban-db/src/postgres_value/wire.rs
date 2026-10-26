pub(super) struct Reader<'a>(pub &'a [u8]);

impl<'a> Reader<'a> {
    pub fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let (value, rest) = self.0.split_at_checked(len)?;
        self.0 = rest;
        Some(value)
    }

    pub fn byte(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }
    pub fn i32(&mut self) -> Option<i32> {
        Some(i32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }
    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }
    pub fn u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(self.take(8)?.try_into().ok()?))
    }
    pub fn f64(&mut self) -> Option<f64> {
        Some(f64::from_bits(self.u64()?))
    }
    pub fn count(&mut self, min_bytes: usize) -> Option<usize> {
        let count = usize::try_from(self.i32()?).ok()?;
        (count <= self.0.len() / min_bytes).then_some(count)
    }
    pub fn value(&mut self) -> Option<Option<&'a [u8]>> {
        match self.i32()? {
            -1 => Some(None),
            len => Some(Some(self.take(usize::try_from(len).ok()?)?)),
        }
    }
    pub fn string(&mut self) -> Option<&'a str> {
        let len = self.0.iter().position(|byte| *byte == 0)?;
        let text = std::str::from_utf8(self.take(len)?).ok()?;
        self.take(1)?;
        Some(text)
    }
    pub fn finish<T>(self, value: T) -> Option<T> {
        self.0.is_empty().then_some(value)
    }
}

pub(super) fn money(raw: &[u8], scale: u32) -> Option<String> {
    if scale > 10 {
        return None;
    }
    let value = i64::from_be_bytes(raw.try_into().ok()?);
    let units = value.unsigned_abs();
    let divisor = 10_u64.pow(scale);
    let sign = if value < 0 { "-" } else { "" };
    Some(if scale == 0 {
        format!("{sign}{units}")
    } else {
        format!(
            "{sign}{}.{:0width$}",
            units / divisor,
            units % divisor,
            width = scale as usize
        )
    })
}

pub(super) fn snapshot(raw: &[u8]) -> Option<String> {
    let mut reader = Reader(raw);
    let count = reader.count(8)?;
    let xmin = reader.u64()?;
    let xmax = reader.u64()?;
    let mut xips = Vec::new();
    for _ in 0..count {
        xips.push(reader.u64()?.to_string());
    }
    reader.finish(format!("{xmin}:{xmax}:{}", xips.join(",")))
}
