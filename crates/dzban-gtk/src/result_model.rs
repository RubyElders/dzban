use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};

use gtk4::gio;
use gtk4::glib;
use gtk4::prelude::*;
use gtk4::subclass::prelude::*;

use crate::database::RuntimeBridge;
use dzban_core::result_stream::ResultHandle;

const CACHE_BYTES: usize = 2 * 1024 * 1024;

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
    result: ResultHandle,
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
    pub fn new(result: ResultHandle, bridge: RuntimeBridge, columns: usize, rows: usize) -> Self {
        let model: Self = glib::Object::new();
        model.imp().source.replace(Some(Source {
            result,
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

    pub fn cached_values(&self) -> Vec<Vec<String>> {
        self.imp()
            .cache
            .borrow()
            .values()
            .filter_map(|row| serde_json::from_str(row).ok())
            .collect()
    }

    pub fn read_row(
        &self,
        position: u32,
    ) -> crate::database::connection_gobject::QueryReceiver<
        dzban_core::ResultSet,
        dzban_core::error::ExecutionError,
    > {
        let source = self.imp().source.borrow();
        let source = source.as_ref().expect("result source initialized");
        source
            .bridge
            .read_result_page(source.result.clone(), position as usize, 1)
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
            return;
        };
        self.imp().loading.replace(Loading::Reading);
        let response = {
            let source = self.imp().source.borrow();
            let source = source.as_ref().expect("result source initialized");
            source
                .bridge
                .read_result_rows(source.result.clone(), start as usize)
        };
        let weak = self.downgrade();
        glib::spawn_future_local(async move {
            let rows = response.await.unwrap_or_else(|error| {
                Err(dzban_core::error::ExecutionError::Message(
                    error.to_string(),
                ))
            });
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
                Err(error) => model.fail(error.to_string()),
            }
        });
    }

    fn fail(&self, error: String) {
        self.imp().loading.replace(Loading::Failed);
        self.imp().pending.borrow_mut().clear();
        self.emit_by_name::<()>("load-error", &[&error]);
    }
}
