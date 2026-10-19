use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use crate::runtime::RuntimeBridge;
use dzban_core::result_store::MAX_PAGE_ROWS;
use dzban_core::{ResultCell, ResultSet};

const CACHE_BYTES: usize = 2 * 1024 * 1024;
const PREVIEW_CHARS: usize = 512;

pub type PageFuture = Pin<Box<dyn Future<Output = Result<ResultSet, String>> + Send>>;

#[derive(Clone)]
pub struct PageReader(Arc<dyn Fn(usize, usize) -> PageFuture + Send + Sync>);

impl PageReader {
    pub fn new(read: impl Fn(usize, usize) -> PageFuture + Send + Sync + 'static) -> Self {
        Self(Arc::new(read))
    }

    pub fn from_output(reader: dzban_core::outputs::ResultReader) -> Self {
        Self::new(move |start, count| {
            let reader = reader.clone();
            Box::pin(async move {
                reader
                    .page(start, count)
                    .await
                    .map_err(|error| error.to_string())
            })
        })
    }

    pub fn from_handle(result: dzban_core::result_stream::ResultHandle) -> Self {
        Self::new(move |start, count| {
            let result = result.clone();
            Box::pin(async move {
                result
                    .read_page(start, count)
                    .await
                    .map_err(|error| error.to_string())
            })
        })
    }

    pub fn page(&self, start: usize, count: usize) -> PageFuture {
        (self.0)(start, count)
    }

    pub fn preview_rows(
        &self,
        start: usize,
    ) -> impl Future<Output = Result<Vec<String>, String>> + Send + 'static + use<> {
        let page = self.page(start, MAX_PAGE_ROWS);
        async move {
            let page = page.await?;
            let mut bytes = 0usize;
            Ok(page
                .rows
                .iter()
                .map(|row| {
                    let chars = (1_048_576usize / row.len().max(1)).saturating_sub(3) / 6;
                    encode_row(row, chars.min(PREVIEW_CHARS))
                })
                .take_while(|row| {
                    if bytes + row.len() > CACHE_BYTES {
                        return false;
                    }
                    bytes += row.len();
                    true
                })
                .collect())
        }
    }
}

pub fn encode_row(row: &[ResultCell], chars: usize) -> String {
    serde_json::to_string(
        &row.iter()
            .map(|cell| match cell {
                ResultCell::Null => None,
                ResultCell::Text(text) => Some(text.chars().take(chars).collect::<String>()),
            })
            .collect::<Vec<_>>(),
    )
    .expect("result preview serialization")
}

pub fn decode_row(row: &str) -> Vec<Option<String>> {
    serde_json::from_str(row).unwrap_or_default()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ResultModelPhase {
    #[default]
    Idle,
    Scheduled,
    Reading,
    Failed,
}

use ResultModelPhase as Loading;

struct Source {
    reader: PageReader,
    bridge: RuntimeBridge,
    placeholder: String,
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ResultListModel {
        pub(super) source: RefCell<Option<Source>>,
        pub(super) rows: Cell<u32>,
        pub(super) cache: RefCell<BTreeMap<u32, String>>,
        pub(super) objects: RefCell<BTreeMap<u32, glib::WeakRef<gtk4::StringObject>>>,
        pub(super) pending: RefCell<BTreeSet<u32>>,
        pub(super) loading: RefCell<Loading>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ResultListModel {
        const NAME: &'static str = "DzbanResultListModel";
        type Type = super::ResultListModel;
        type Interfaces = (gio::ListModel,);
    }

    impl ObjectImpl for ResultListModel {
        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: std::sync::OnceLock<Vec<glib::subclass::Signal>> =
                std::sync::OnceLock::new();
            SIGNALS.get_or_init(|| {
                vec![
                    glib::subclass::Signal::builder("page-ready").build(),
                    glib::subclass::Signal::builder("idle").build(),
                    glib::subclass::Signal::builder("load-error")
                        .param_types([String::static_type()])
                        .build(),
                ]
            })
        }
    }

    impl ListModelImpl for ResultListModel {
        fn item_type(&self) -> glib::Type {
            gtk4::StringObject::static_type()
        }
        fn n_items(&self) -> u32 {
            self.rows.get()
        }
        fn item(&self, position: u32) -> Option<glib::Object> {
            if position >= self.rows.get() {
                return None;
            }
            if let Some(object) = self
                .objects
                .borrow()
                .get(&position)
                .and_then(glib::WeakRef::upgrade)
            {
                return Some(object.upcast());
            }
            if let Some(row) = self.cache.borrow().get(&position) {
                let object = gtk4::StringObject::new(row);
                self.objects
                    .borrow_mut()
                    .insert(position, object.downgrade());
                return Some(object.upcast());
            }
            self.obj().request(position);
            self.source.borrow().as_ref().map(|source| {
                let object = gtk4::StringObject::new(&source.placeholder);
                self.objects
                    .borrow_mut()
                    .insert(position, object.downgrade());
                object.upcast()
            })
        }
    }
}

glib::wrapper! {
    pub struct ResultListModel(ObjectSubclass<imp::ResultListModel>) @implements gio::ListModel;
}

impl ResultListModel {
    pub fn new(reader: PageReader, bridge: RuntimeBridge, columns: usize, rows: usize) -> Self {
        let model: Self = glib::Object::new();
        model.imp().source.replace(Some(Source {
            reader,
            bridge,
            placeholder: serde_json::to_string(&vec![""; columns])
                .expect("result placeholder serialization"),
        }));
        model.set_rows(rows);
        model
    }

    pub fn set_rows(&self, rows: usize) {
        let rows = u32::try_from(rows).expect("bounded result row count");
        let previous = self.imp().rows.replace(rows);
        self.imp()
            .objects
            .borrow_mut()
            .retain(|position, object| *position < rows && object.upgrade().is_some());
        self.imp()
            .cache
            .borrow_mut()
            .retain(|position, _| *position < rows);
        self.imp()
            .pending
            .borrow_mut()
            .retain(|position| *position < rows);
        if rows > previous {
            self.items_changed(previous, 0, rows - previous);
        } else if rows < previous {
            self.items_changed(rows, previous - rows, 0);
        }
    }

    pub fn cached_rows(&self) -> usize {
        self.imp().cache.borrow().len()
    }

    pub fn phase(&self) -> ResultModelPhase {
        *self.imp().loading.borrow()
    }

    pub fn cached_bytes(&self) -> usize {
        self.imp().cache.borrow().values().map(String::len).sum()
    }

    pub fn cached_values(&self) -> Vec<Vec<Option<String>>> {
        self.imp()
            .cache
            .borrow()
            .values()
            .map(|row| decode_row(row))
            .collect()
    }

    pub fn read_page(
        &self,
        start: usize,
        count: usize,
    ) -> impl Future<Output = Result<ResultSet, String>> + 'static + use<> {
        let (reader, bridge) = {
            let source = self.imp().source.borrow();
            let source = source.as_ref().expect("result source initialized");
            (source.reader.clone(), source.bridge.clone())
        };
        let response = bridge.run(reader.page(start, count));
        async move { response.await.map_err(|error| error.to_string())? }
    }

    fn request(&self, position: u32) {
        if matches!(*self.imp().loading.borrow(), Loading::Failed) {
            return;
        }
        self.imp().pending.borrow_mut().insert(position);
        if !matches!(*self.imp().loading.borrow(), Loading::Idle) {
            return;
        }
        self.imp().loading.replace(Loading::Scheduled);
        let weak = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(model) = weak.upgrade() {
                model.load_next();
            }
        });
    }

    fn load_next(&self) {
        let Some(start) = self.imp().pending.borrow_mut().pop_first() else {
            self.imp().loading.replace(Loading::Idle);
            self.emit_by_name::<()>("idle", &[]);
            return;
        };
        self.imp().loading.replace(Loading::Reading);
        let response = {
            let source = self.imp().source.borrow();
            let source = source.as_ref().expect("result source initialized");
            source
                .bridge
                .run(source.reader.preview_rows(start as usize))
        };
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let rows = response
                .await
                .unwrap_or_else(|error| Err(error.to_string()));
            let Some(model) = weak.upgrade() else {
                return;
            };
            if start >= model.n_items() {
                model.load_next();
                return;
            }
            match rows {
                Ok(rows) if !rows.is_empty() => {
                    let count = rows.len() as u32;
                    let mut bytes = 0usize;
                    let cache = rows
                        .into_iter()
                        .enumerate()
                        .take_while(|(_, row)| {
                            let fits = bytes == 0 || bytes.saturating_add(row.len()) <= CACHE_BYTES;
                            if fits {
                                bytes += row.len();
                            }
                            fits
                        })
                        .map(|(offset, row)| (start + offset as u32, row))
                        .collect::<BTreeMap<_, _>>();
                    let end = cache.last_key_value().map_or(start, |(last, _)| last + 1);
                    model.imp().cache.replace(cache);
                    model.imp().objects.borrow_mut().retain(|position, object| {
                        (*position < start || *position >= end) && object.upgrade().is_some()
                    });
                    model
                        .imp()
                        .pending
                        .borrow_mut()
                        .retain(|position| *position < start || *position >= end);
                    let visible_count = count
                        .min(end - start)
                        .min(model.n_items().saturating_sub(start));
                    model.items_changed(start, visible_count, visible_count);
                    model.emit_by_name::<()>("page-ready", &[]);
                    model.load_next();
                }
                Ok(_) => model.fail("Result page is missing".into()),
                Err(error) => model.fail(error),
            }
        });
    }

    fn fail(&self, error: String) {
        self.imp().loading.replace(Loading::Failed);
        self.imp().pending.borrow_mut().clear();
        self.emit_by_name::<()>("load-error", &[&error]);
    }
}
