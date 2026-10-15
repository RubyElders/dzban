use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::{service::App, transport::Stream};

pub enum Handle {
    App(Arc<App>),
    Stream(Arc<Stream>),
}

#[derive(Default)]
struct Registry {
    next: u64,
    handles: HashMap<u64, Handle>,
    buffers: HashMap<u64, Box<[u8]>>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Mutex::default)
}

fn insert(handle: Handle) -> Option<u64> {
    let mut registry = registry().lock().unwrap();
    if registry.handles.len() >= 4096 {
        return None;
    }
    registry.next = registry.next.checked_add(1)?;
    let id = registry.next;
    registry.handles.insert(id, handle);
    Some(id)
}

pub fn insert_app(app: App) -> Option<u64> {
    insert(Handle::App(Arc::new(app)))
}

pub fn insert_stream(stream: Arc<Stream>) -> Option<u64> {
    insert(Handle::Stream(stream))
}

pub fn app(id: u64) -> Option<Arc<App>> {
    match registry().lock().unwrap().handles.get(&id)? {
        Handle::App(app) => Some(app.clone()),
        _ => None,
    }
}

pub fn stream(id: u64) -> Option<Arc<Stream>> {
    match registry().lock().unwrap().handles.get(&id)? {
        Handle::Stream(stream) => Some(stream.clone()),
        _ => None,
    }
}

pub fn release(id: u64) -> bool {
    let mut registry = registry().lock().unwrap();
    if let Some(Handle::App(app)) = registry.handles.get(&id)
        && !app.is_closed()
    {
        return false;
    }
    let handle = registry.handles.remove(&id);
    drop(registry);
    match handle {
        Some(Handle::Stream(stream)) => stream.close(),
        Some(Handle::App(_)) => (),
        None => return false,
    }
    true
}

pub fn buffer(bytes: Vec<u8>) -> Option<(u64, *const u8, usize)> {
    let mut registry = registry().lock().unwrap();
    if registry.buffers.len() >= 64 {
        return None;
    }
    registry.next = registry.next.checked_add(1)?;
    let id = registry.next;
    let bytes = bytes.into_boxed_slice();
    let pointer = bytes.as_ptr();
    let length = bytes.len();
    registry.buffers.insert(id, bytes);
    Some((id, pointer, length))
}

pub fn release_buffer(id: u64) -> bool {
    registry().lock().unwrap().buffers.remove(&id).is_some()
}
