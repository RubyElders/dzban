use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

use crate::{ResultCell, ResultColumn};

pub const MAX_PAGE_ROWS: usize = 200;

#[derive(Clone, Copy, Debug)]
pub struct ResultLimits {
    pub rows: usize,
    pub bytes: usize,
    pub memory_rows: usize,
    pub memory_bytes: usize,
}

impl Default for ResultLimits {
    fn default() -> Self {
        Self {
            rows: 10_000,
            bytes: 32 * 1024 * 1024,
            memory_rows: 200,
            memory_bytes: 2 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ResultCutoff {
    Rows,
    Bytes,
}

enum StoredRow {
    Memory(Vec<ResultCell>),
    Disk {
        offset: u64,
        length: usize,
        bytes: usize,
    },
}

fn encode_row(row: &[ResultCell], limit: usize) -> io::Result<Option<Vec<u8>>> {
    struct Buffer {
        bytes: Vec<u8>,
        limit: usize,
        exceeded: bool,
    }
    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.limit - self.bytes.len() {
                self.exceeded = true;
                return Err(io::Error::other("Result encoding exceeds storage limits"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut buffer = Buffer {
        bytes: Vec::new(),
        limit,
        exceeded: false,
    };
    let result = serde_json::to_writer(&mut buffer, row);
    if buffer.exceeded {
        return Ok(None);
    }
    result?;
    Ok(Some(buffer.bytes))
}

struct Spool {
    file: Option<File>,
    path: PathBuf,
}

impl Spool {
    fn create() -> io::Result<Self> {
        let path = std::env::temp_dir().join(format!("dzban-results-{}", uuid::Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        Ok(Self {
            file: Some(options.open(&path)?),
            path,
        })
    }
}

impl Drop for Spool {
    fn drop(&mut self) {
        self.file.take();
        if let Err(error) = std::fs::remove_file(&self.path) {
            tracing::warn!(%error, "Could not remove temporary result spool");
        }
    }
}

pub struct ResultStore {
    columns: Vec<ResultColumn>,
    rows: Vec<StoredRow>,
    limits: ResultLimits,
    bytes: usize,
    memory_bytes: usize,
    memory_rows: usize,
    cutoff: Option<ResultCutoff>,
    spool: Option<Spool>,
}

impl ResultStore {
    pub fn new(columns: Vec<ResultColumn>, limits: ResultLimits) -> io::Result<Self> {
        let bytes = columns
            .iter()
            .try_fold(0usize, |bytes, column| {
                bytes
                    .checked_add(column.name.len())?
                    .checked_add(column.type_name.len())
            })
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "Result schema size overflow")
            })?;
        if limits.rows == 0 || bytes > limits.bytes || bytes > limits.memory_bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Result schema exceeds storage limits",
            ));
        }
        Ok(Self {
            columns,
            rows: Vec::new(),
            limits,
            bytes,
            memory_bytes: bytes,
            memory_rows: 0,
            cutoff: (bytes == limits.bytes).then_some(ResultCutoff::Bytes),
            spool: None,
        })
    }

    pub fn columns(&self) -> &[ResultColumn] {
        &self.columns
    }
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn cutoff(&self) -> Option<ResultCutoff> {
        self.cutoff
    }

    pub(crate) fn reject_oversized_row(&mut self) -> ResultCutoff {
        *self.cutoff.get_or_insert(ResultCutoff::Bytes)
    }

    pub fn push(&mut self, row: Vec<ResultCell>) -> io::Result<Option<ResultCutoff>> {
        if let Some(cutoff) = self.cutoff {
            return Ok(Some(cutoff));
        }
        if row.len() != self.columns.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Result row does not match its schema",
            ));
        }
        let row_bytes = row
            .iter()
            .try_fold(0usize, |bytes, cell| {
                bytes.checked_add(cell.display().len())
            })
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "Result row size overflow")
            })?;
        if row_bytes > self.limits.bytes - self.bytes {
            self.cutoff = Some(ResultCutoff::Bytes);
            return Ok(self.cutoff);
        }
        let stored = if self.memory_rows < self.limits.memory_rows
            && self.spool.is_none()
            && row_bytes <= self.limits.memory_bytes - self.memory_bytes
        {
            self.memory_rows += 1;
            self.memory_bytes += row_bytes;
            StoredRow::Memory(row)
        } else {
            if self.spool.is_none() {
                self.spool = Some(Spool::create()?);
            }
            let file = self.spool.as_mut().unwrap().file.as_mut().unwrap();
            let offset = file.seek(SeekFrom::End(0))?;
            let remaining = self
                .limits
                .bytes
                .saturating_sub(self.memory_bytes)
                .saturating_sub(usize::try_from(offset).unwrap_or(usize::MAX));
            let Some(encoded) = encode_row(&row, remaining)? else {
                self.cutoff = Some(ResultCutoff::Bytes);
                return Ok(self.cutoff);
            };
            file.write_all(&encoded)?;
            StoredRow::Disk {
                offset,
                length: encoded.len(),
                bytes: row_bytes,
            }
        };
        self.rows.push(stored);
        self.bytes += row_bytes;
        self.cutoff = if self.rows.len() == self.limits.rows {
            Some(ResultCutoff::Rows)
        } else if self.bytes == self.limits.bytes {
            Some(ResultCutoff::Bytes)
        } else {
            None
        };
        Ok(self.cutoff)
    }

    pub fn read_page(&mut self, start: usize, count: usize) -> io::Result<Vec<Vec<ResultCell>>> {
        if count > MAX_PAGE_ROWS {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Result page is too large",
            ));
        }
        let mut page = Vec::new();
        let mut bytes = self
            .columns
            .iter()
            .map(|column| column.name.len() + column.type_name.len())
            .sum::<usize>();
        for row in self.rows.iter().skip(start).take(count) {
            let row_bytes = match row {
                StoredRow::Memory(row) => {
                    row.iter().map(|cell| cell.display().len()).sum::<usize>()
                }
                StoredRow::Disk { bytes, .. } => *bytes,
            };
            if !page.is_empty() && bytes.saturating_add(row_bytes) > self.limits.memory_bytes {
                break;
            }
            bytes = bytes.saturating_add(row_bytes);
            let row = match row {
                StoredRow::Memory(row) => row.clone(),
                StoredRow::Disk { offset, length, .. } => {
                    let file = self.spool.as_mut().unwrap().file.as_mut().unwrap();
                    file.seek(SeekFrom::Start(*offset))?;
                    let mut encoded = vec![0; *length];
                    file.read_exact(&mut encoded)?;
                    serde_json::from_slice(&encoded)?
                }
            };
            page.push(row);
        }
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns() -> Vec<ResultColumn> {
        vec![ResultColumn {
            name: "value".into(),
            type_name: "text".into(),
        }]
    }

    #[test]
    fn disk_pages_preserve_order_null_and_unicode_and_cleanup() {
        let mut store = ResultStore::new(
            columns(),
            ResultLimits {
                memory_rows: 1,
                ..Default::default()
            },
        )
        .unwrap();
        let rows = vec![
            vec![ResultCell::Text("first".into())],
            vec![ResultCell::Null],
            vec![ResultCell::Text("žluť\nline\t\"".into())],
        ];
        for row in &rows {
            assert_eq!(store.push(row.clone()).unwrap(), None);
        }
        assert_eq!(store.memory_rows, 1);
        assert_eq!(store.read_page(0, 3).unwrap(), rows);
        assert_eq!(store.read_page(2, 1).unwrap(), rows[2..]);
        let path = store.spool.as_ref().unwrap().path.clone();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(store);
        assert!(!path.exists());
    }

    #[test]
    fn limits_count_schema_utf8_and_null_display_bytes() {
        let mut store = ResultStore::new(
            columns(),
            ResultLimits {
                bytes: 15,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(store.bytes(), 9);
        assert_eq!(
            store.push(vec![ResultCell::Text("ž".into())]).unwrap(),
            None
        );
        assert_eq!(
            store.push(vec![ResultCell::Null]).unwrap(),
            Some(ResultCutoff::Bytes)
        );
        assert_eq!(store.bytes(), 15);
        assert_eq!(store.len(), 2);
        assert_eq!(
            store
                .push(vec![ResultCell::Text("ignored".into())])
                .unwrap(),
            Some(ResultCutoff::Bytes)
        );
        assert_eq!(store.len(), 2);
        let mut store = ResultStore::new(
            columns(),
            ResultLimits {
                rows: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            store.push(vec![ResultCell::Null]).unwrap(),
            Some(ResultCutoff::Rows)
        );
    }

    #[test]
    fn oversized_rows_schema_and_pages_cannot_bypass_limits() {
        let mut store = ResultStore::new(
            columns(),
            ResultLimits {
                bytes: 10,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            store
                .push(vec![ResultCell::Text("too large".into())])
                .unwrap(),
            Some(ResultCutoff::Bytes)
        );
        assert!(store.is_empty());
        assert!(store.spool.is_none());
        assert!(store.read_page(0, MAX_PAGE_ROWS + 1).is_err());
        assert!(
            ResultStore::new(
                columns(),
                ResultLimits {
                    bytes: 8,
                    ..Default::default()
                }
            )
            .is_err()
        );
        assert!(
            ResultStore::new(
                columns(),
                ResultLimits {
                    memory_bytes: 8,
                    ..Default::default()
                }
            )
            .is_err()
        );
        let mut store = ResultStore::new(columns(), ResultLimits::default()).unwrap();
        assert!(store.push(Vec::new()).is_err());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn encoded_control_characters_cannot_expand_the_spool_past_its_cap() {
        let limits = ResultLimits {
            bytes: 32,
            memory_rows: 0,
            ..Default::default()
        };
        let mut store = ResultStore::new(columns(), limits).unwrap();
        assert_eq!(
            store.push(vec![ResultCell::Text("\0\0\0".into())]).unwrap(),
            None
        );
        assert_eq!(
            store.push(vec![ResultCell::Text("\0\0\0".into())]).unwrap(),
            Some(ResultCutoff::Bytes)
        );
        assert_eq!(store.len(), 1);
        assert!(
            std::fs::metadata(&store.spool.as_ref().unwrap().path)
                .unwrap()
                .len()
                <= 32
        );
        assert_eq!(
            store.read_page(0, 1).unwrap(),
            vec![vec![ResultCell::Text("\0\0\0".into())]]
        );
    }

    #[test]
    fn memory_retention_cannot_resume_after_spilling_and_bypass_the_budget() {
        let mut store = ResultStore::new(
            columns(),
            ResultLimits {
                bytes: 32,
                memory_bytes: 11,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            store.push(vec![ResultCell::Text("\0\0\0".into())]).unwrap(),
            None
        );
        assert_eq!(
            store.push(vec![ResultCell::Text("xx".into())]).unwrap(),
            Some(ResultCutoff::Bytes)
        );
        assert_eq!(store.len(), 1);
        assert_eq!(store.memory_rows, 0);
    }
}
