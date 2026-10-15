mod editor;
#[cfg(feature = "test-fixtures")]
mod fixture;
mod handles;
mod service;
mod transport;
mod viewport;

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
        let command = if data.is_null() || length > 1024 * 1024 {
            None
        } else {
            let bytes = unsafe { std::slice::from_raw_parts(data, length) };
            serde_json::from_slice::<service::Command>(bytes).ok()
        };
        if let Some(command) = command {
            app.dispatch(command, output);
        } else {
            output.publish(json!({"failure": {"kind": "invalid_input", "message": "Invalid command or request exceeds 1 MiB"}}));
        }
        handle
    })).unwrap_or(0)
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
