mod catalog;
mod editor;
#[cfg(feature = "test-fixtures")]
mod fixture;
mod handles;
mod service;
mod transport;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::Duration;

use serde_json::json;

use transport::{Stream, Wait};

#[repr(C)]
pub struct BridgeBuffer {
    pub status: u32,
    pub token: u64,
    pub data: *const u8,
    pub length: usize,
}

impl BridgeBuffer {
    fn status(status: u32) -> Self {
        Self {
            status,
            token: 0,
            data: std::ptr::null(),
            length: 0,
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn dzban_abi_version() -> u32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn dzban_create() -> u64 {
    catch_unwind(|| {
        service::App::new(dzban_db::adapters::registry(), None)
            .ok()
            .and_then(handles::insert_app)
            .unwrap_or(0)
    })
    .unwrap_or(0)
}

#[cfg(feature = "test-fixtures")]
#[unsafe(no_mangle)]
pub extern "C" fn dzban_create_fixture() -> u64 {
    catch_unwind(|| {
        fixture::app()
            .ok()
            .and_then(handles::insert_app)
            .unwrap_or(0)
    })
    .unwrap_or(0)
}

/// # Safety
/// `data` must point to `length` readable bytes for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dzban_send(app: u64, data: *const u8, length: usize) -> u64 {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(app) = handles::app(app) else { return 0; };
        let output = Stream::new();
        let Some(handle) = handles::insert_stream(output.clone()) else { return 0; };
        let command = if data.is_null() || length > 32 * 1024 * 1024 {
            None
        } else {
            let bytes = unsafe { std::slice::from_raw_parts(data, length) };
            serde_json::from_slice::<service::Command>(bytes).ok().filter(|command| length <= 1024 * 1024 || matches!(command, service::Command::SaveWorkspace { .. }))
        };
        if let Some(command) = command {
            app.dispatch(command, output);
        } else {
            output.publish(json!({"failure": {"kind": "invalid_input", "message": "Invalid command or request exceeds its size limit"}}));
        }
        handle
    })).unwrap_or(0)
}

/// # Safety
/// `data` must point to `length` readable bytes, or be null when `length` is zero.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dzban_syntax_create(data: *const u8, length: usize, revision: u64) -> u64 {
    catch_unwind(AssertUnwindSafe(|| {
        if (data.is_null() && length != 0) || length > 32 * 1024 * 1024 {
            return 0;
        }
        let bytes = if length == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(data, length) }
        };
        let Ok(source) = std::str::from_utf8(bytes) else {
            return 0;
        };
        handles::insert_syntax(dzban_core::sql::SqlLexer::new(source, revision)).unwrap_or(0)
    }))
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn dzban_syntax_read(handle: u64) -> BridgeBuffer {
    catch_unwind(|| {
        let Some(lexer) = handles::syntax(handle) else {
            return BridgeBuffer::status(4);
        };
        let batch = lexer.lock().unwrap().read(512);
        let mut record = Vec::with_capacity(56 + batch.tokens.len() * 56);
        for word in [
            1,
            batch.revision,
            batch.length.bytes as u64,
            batch.length.scalars as u64,
            batch.length.utf16 as u64,
            batch.tokens.len() as u64,
            u64::from(batch.done),
        ] {
            record.extend_from_slice(&word.to_le_bytes());
        }
        for token in batch.tokens {
            for word in [
                token.start.bytes as u64,
                token.start.scalars as u64,
                token.start.utf16 as u64,
                token.end.bytes as u64,
                token.end.scalars as u64,
                token.end.utf16 as u64,
                token.kind as u64,
            ] {
                record.extend_from_slice(&word.to_le_bytes());
            }
        }
        match handles::buffer(record) {
            Some((token, data, length)) => BridgeBuffer {
                status: 0,
                token,
                data,
                length,
            },
            None => BridgeBuffer::status(6),
        }
    })
    .unwrap_or_else(|_| BridgeBuffer::status(5))
}

/// # Safety
/// `data` must point to `length` readable bytes for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dzban_attention(app: u64, data: *const u8, length: usize) -> BridgeBuffer {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(app) = handles::app(app) else {
            return BridgeBuffer::status(4);
        };
        if data.is_null() || length > 1024 * 1024 {
            return BridgeBuffer::status(7);
        }
        let bytes = unsafe { std::slice::from_raw_parts(data, length) };
        let Ok(input) = serde_json::from_slice::<dzban_core::attention::AttentionInput>(bytes)
        else {
            return BridgeBuffer::status(7);
        };
        let snapshot = app.attention_input(input);
        let record = serde_json::to_vec(
            &json!({"schema":1,"revision":snapshot.revision,"payload":snapshot}),
        )
        .unwrap();
        if record.len() > transport::MAX_RECORD_BYTES {
            return BridgeBuffer::status(6);
        }
        match handles::buffer(record) {
            Some((token, data, length)) => BridgeBuffer {
                status: 0,
                token,
                data,
                length,
            },
            None => BridgeBuffer::status(6),
        }
    }))
    .unwrap_or_else(|_| BridgeBuffer::status(5))
}

/// # Safety
/// `data` must point to `length` readable bytes for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn dzban_result_view(data: *const u8, length: usize) -> BridgeBuffer {
    catch_unwind(AssertUnwindSafe(|| {
        if data.is_null() || length > 32 * 1024 * 1024 {
            return BridgeBuffer::status(7);
        }
        let bytes = unsafe { std::slice::from_raw_parts(data, length) };
        let Ok(request) =
            serde_json::from_slice::<dzban_core::outputs::view::ResultViewRequest>(bytes)
        else {
            return BridgeBuffer::status(7);
        };
        let record =
            serde_json::to_vec(&json!({"schema":1,"revision":0,"payload":request.evaluate()}))
                .unwrap();
        match handles::buffer(record) {
            Some((token, data, length)) => BridgeBuffer {
                status: 0,
                token,
                data,
                length,
            },
            None => BridgeBuffer::status(6),
        }
    }))
    .unwrap_or_else(|_| BridgeBuffer::status(5))
}

#[unsafe(no_mangle)]
pub extern "C" fn dzban_wait(stream: u64, after_revision: u64, timeout_ms: u32) -> BridgeBuffer {
    catch_unwind(|| {
        let Some(stream) = handles::stream(stream) else {
            return BridgeBuffer::status(4);
        };
        match stream.wait(
            after_revision,
            Duration::from_millis(u64::from(timeout_ms.min(60_000))),
        ) {
            Wait::Record(record) => match handles::buffer(record) {
                Some((token, data, length)) => BridgeBuffer {
                    status: 0,
                    token,
                    data,
                    length,
                },
                None => BridgeBuffer::status(6),
            },
            Wait::Timeout => BridgeBuffer::status(1),
            Wait::Closed => BridgeBuffer::status(2),
            Wait::AlreadyWaiting => BridgeBuffer::status(3),
        }
    })
    .unwrap_or_else(|_| BridgeBuffer::status(5))
}

#[unsafe(no_mangle)]
pub extern "C" fn dzban_release(handle: u64) -> bool {
    catch_unwind(|| handles::release(handle)).unwrap_or(false)
}

#[unsafe(no_mangle)]
pub extern "C" fn dzban_buffer_release(token: u64) -> bool {
    catch_unwind(|| handles::release_buffer(token)).unwrap_or(false)
}
