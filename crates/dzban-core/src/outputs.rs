mod export;
mod history;
pub mod view;

pub use export::{ExportChunk, OutputExport, RowRange};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::application::{ApplicationContext, ApplicationError};
use crate::documents::{DocumentId, DocumentLifecycle, QueryDocument};
use crate::error::ExecutionError;
use crate::execution::SessionId;
use crate::history::QueryHistoryStore;
use crate::presentation::{
    DocumentPresentation, HistoryPresentation, PlanPresentation, WorkspacePresentation,
};
use crate::result_stream::ResultHandle;
use crate::{ResultCell, ResultSet};

pub const MAX_VIEW_COLUMNS: usize = 16;
pub const CELL_PREVIEW_SCALARS: usize = 256;
pub const MAX_CELL_CHUNK_SCALARS: usize = 65_536;
pub const ORIGINAL_CELL_BYTES: usize = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResultScope {
    pub session: SessionId,
    pub document: DocumentId,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OutputError {
    Application(ApplicationError),
    UnknownDocument,
    StaleOutput,
    Unavailable,
    StaleHistory,
    HistoryEntryMissing,
    SavedOutputMissing,
    InvalidCell,
    InvalidPage(&'static str),
    InvalidChunk(&'static str),
    CellTooLarge,
    ExportTooLarge,
    InvalidSelection,
    Execution(ExecutionError),
    History(crate::commands::CommandError),
}

impl std::fmt::Display for OutputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Application(error) => error.fmt(formatter),
            Self::UnknownDocument => formatter.write_str("Document is not open"),
            Self::StaleOutput => formatter.write_str("Output generation has changed"),
            Self::Unavailable => formatter.write_str("Result is not available"),
            Self::StaleHistory => formatter.write_str("History has been refreshed"),
            Self::HistoryEntryMissing => formatter.write_str("History entry is missing"),
            Self::SavedOutputMissing => formatter.write_str("Saved output is not selected"),
            Self::InvalidCell => formatter.write_str("Cell is outside the result"),
            Self::InvalidPage(message) | Self::InvalidChunk(message) => {
                formatter.write_str(message)
            }
            Self::CellTooLarge => formatter.write_str("Cell exceeds the 32 MiB limit"),
            Self::ExportTooLarge => {
                formatter.write_str("Results exceed the 32 MiB clipboard limit")
            }
            Self::InvalidSelection => formatter.write_str("Result selection is outside the output"),
            Self::Execution(error) => error.fmt(formatter),
            Self::History(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for OutputError {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellPreview {
    pub text: Option<String>,
    pub length: usize,
    pub truncated: bool,
}

impl From<&ResultCell> for CellPreview {
    fn from(cell: &ResultCell) -> Self {
        match cell {
            ResultCell::Null => Self {
                text: None,
                length: 0,
                truncated: false,
            },
            ResultCell::Text(text) => {
                let length = text.chars().count();
                Self {
                    text: Some(text.chars().take(CELL_PREVIEW_SCALARS).collect()),
                    length,
                    truncated: length > CELL_PREVIEW_SCALARS,
                }
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewportPage {
    pub document: Uuid,
    pub generation: u64,
    pub start: usize,
    pub column_start: usize,
    pub rows: Vec<Vec<CellPreview>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellChunk {
    pub document: Uuid,
    pub generation: u64,
    pub row: usize,
    pub column: usize,
    pub offset: usize,
    pub total: usize,
    pub text: Option<String>,
}

#[derive(Default)]
struct OutputData {
    history_views: HashMap<SessionId, history::HistoryView>,
    saved: HashMap<DocumentId, history::SavedOutput>,
}

#[derive(Clone, Default)]
pub struct OutputService {
    data: Arc<Mutex<OutputData>>,
}

#[derive(Clone)]
enum Source {
    Live(ResultHandle),
    Saved(history::SavedOutput),
}

#[derive(Clone)]
pub struct ResultReader {
    scope: ResultScope,
    document: QueryDocument,
    source: Source,
}

impl OutputService {
    fn list_history(
        &self,
        session: SessionId,
        document: &QueryDocument,
        store: QueryHistoryStore,
    ) -> Result<HistoryPresentation, OutputError> {
        let mut data = self.data.lock().unwrap();
        if document.snapshot().lifecycle != DocumentLifecycle::Open {
            return Err(OutputError::StaleOutput);
        }
        let view = history::HistoryView {
            revision: Uuid::new_v4(),
            store: Arc::new(store),
        };
        let response = history::summaries(&view);
        data.history_views.insert(session, view);
        Ok(response)
    }

    fn restore_history(
        &self,
        session: SessionId,
        document: &QueryDocument,
        revision: Uuid,
        index: usize,
    ) -> Result<DocumentPresentation, OutputError> {
        let mut data = self.data.lock().unwrap();
        if document.snapshot().lifecycle != DocumentLifecycle::Open {
            return Err(OutputError::StaleOutput);
        }
        let store = data
            .history_views
            .get(&session)
            .filter(|view| view.revision == revision)
            .ok_or(OutputError::StaleHistory)?
            .store
            .clone();
        let entry = store
            .entries()
            .get(index)
            .ok_or(OutputError::HistoryEntryMissing)?;
        let restored = document
            .replace_draft_and_output(entry.sql.clone())
            .ok_or(OutputError::StaleOutput)?;
        let generation = restored.output_generation;
        let output = history::SavedOutput {
            generation,
            store,
            index,
        };
        let snapshot = output.project(DocumentPresentation::from(&restored));
        data.saved.insert(document.id(), output);
        let weak = Arc::downgrade(&self.data);
        let id = document.id();
        let mut snapshots = document.subscribe();
        tokio::spawn(async move {
            loop {
                let snapshot = snapshots.borrow_and_update().clone();
                if snapshot.lifecycle != DocumentLifecycle::Open
                    || snapshot.output_generation != generation
                {
                    if let Some(data) = weak.upgrade() {
                        let mut data = data.lock().unwrap();
                        if data
                            .saved
                            .get(&id)
                            .is_some_and(|output| output.generation == generation)
                        {
                            data.saved.remove(&id);
                        }
                    }
                    return;
                }
                if snapshots.changed().await.is_err() {
                    return;
                }
            }
        });
        Ok(snapshot)
    }

    pub fn document(&self, document: &QueryDocument) -> DocumentPresentation {
        let snapshot = DocumentPresentation::from(&document.snapshot());
        self.project(document.id(), snapshot)
    }

    fn project(
        &self,
        document: DocumentId,
        snapshot: DocumentPresentation,
    ) -> DocumentPresentation {
        let data = self.data.lock().unwrap();
        match data
            .saved
            .get(&document)
            .filter(|saved| snapshot.generation == saved.generation)
        {
            Some(saved) => saved.project(snapshot),
            None => snapshot,
        }
    }

    pub fn workspace(&self, snapshot: &crate::tabs::TabsSnapshot) -> WorkspacePresentation {
        let mut inventory = WorkspacePresentation::from(snapshot);
        let data = self.data.lock().unwrap();
        for tab in &mut inventory.tabs {
            if let Some(output) = data
                .saved
                .get(&DocumentId(tab.document))
                .filter(|output| tab.snapshot.generation == output.generation)
            {
                tab.snapshot = output.project(tab.snapshot.clone());
            }
        }
        inventory
    }

    fn saved_plan(
        &self,
        document: &QueryDocument,
        generation: u64,
    ) -> Result<Option<PlanPresentation>, OutputError> {
        if !document.accepts_output(generation) {
            return Err(OutputError::StaleOutput);
        }
        let data = self.data.lock().unwrap();
        let output = data
            .saved
            .get(&document.id())
            .filter(|saved| saved.generation == generation)
            .ok_or(OutputError::SavedOutputMissing)?;
        Ok(output.plan().map(PlanPresentation::from_saved))
    }

    pub(crate) fn retire_session(&self, session: SessionId, document: Option<DocumentId>) {
        let mut data = self.data.lock().unwrap();
        data.history_views.remove(&session);
        if let Some(document) = document {
            data.saved.remove(&document);
        }
    }

    pub(crate) fn clear(&self) {
        *self.data.lock().unwrap() = Default::default();
    }

    fn reader(
        &self,
        scope: ResultScope,
        document: QueryDocument,
    ) -> Result<ResultReader, OutputError> {
        let snapshot = document.snapshot();
        if document.id() != scope.document {
            return Err(OutputError::UnknownDocument);
        }
        if !document.accepts_output(scope.generation) {
            return Err(OutputError::StaleOutput);
        }
        let data = self.data.lock().unwrap();
        let source = match data
            .saved
            .get(&scope.document)
            .filter(|saved| saved.generation == scope.generation)
        {
            Some(saved) => Source::Saved(saved.clone()),
            None if snapshot.execution_generation == scope.generation => Source::Live(
                snapshot
                    .operation
                    .and_then(|operation| operation.capture)
                    .and_then(|capture| capture.result)
                    .ok_or(OutputError::Unavailable)?,
            ),
            None => return Err(OutputError::Unavailable),
        };
        Ok(ResultReader {
            scope,
            document,
            source,
        })
    }
}

impl ApplicationContext {
    fn output_document(
        &self,
        session: SessionId,
        document: DocumentId,
    ) -> Result<QueryDocument, OutputError> {
        let tab = self.tabs.session(session).ok_or(OutputError::Application(
            ApplicationError::UnknownSession(session),
        ))?;
        if tab.document.id() != document {
            return Err(OutputError::UnknownDocument);
        }
        Ok(tab.document)
    }

    pub async fn history_list(
        &self,
        session: SessionId,
        location: crate::commands::HistoryLocation,
    ) -> Result<HistoryPresentation, OutputError> {
        let tab = self.tabs.session(session).ok_or(OutputError::Application(
            ApplicationError::UnknownSession(session),
        ))?;
        let profile = tab
            .profile
            .ok_or(OutputError::Application(ApplicationError::Operation(
                ExecutionError::Closed,
            )))?;
        let store = location
            .load(profile.0)
            .await
            .map_err(OutputError::History)?;
        self.outputs.list_history(session, &tab.document, store)
    }

    pub fn restore_history(
        &self,
        session: SessionId,
        document: DocumentId,
        revision: Uuid,
        index: usize,
    ) -> Result<DocumentPresentation, OutputError> {
        let document = self.output_document(session, document)?;
        self.outputs
            .restore_history(session, &document, revision, index)
    }

    pub fn saved_plan(&self, scope: ResultScope) -> Result<Option<PlanPresentation>, OutputError> {
        let document = self.output_document(scope.session, scope.document)?;
        self.outputs.saved_plan(&document, scope.generation)
    }

    pub fn result_reader(&self, scope: ResultScope) -> Result<ResultReader, OutputError> {
        let document = self.output_document(scope.session, scope.document)?;
        self.outputs.reader(scope, document)
    }
}

impl ResultReader {
    pub fn scope(&self) -> ResultScope {
        self.scope
    }

    pub fn validate(&self) -> Result<(), OutputError> {
        if self.document.accepts_output(self.scope.generation) {
            Ok(())
        } else {
            Err(OutputError::StaleOutput)
        }
    }

    pub async fn page(&self, start: usize, count: usize) -> Result<ResultSet, OutputError> {
        self.validate()?;
        if count > crate::result_store::MAX_PAGE_ROWS {
            return Err(OutputError::InvalidPage("Page exceeds MAX_PAGE_ROWS"));
        }
        let page = match &self.source {
            Source::Live(result) => result
                .read_page(start, count)
                .await
                .map_err(OutputError::Execution)?,
            Source::Saved(saved) => saved.page(start, count),
        };
        self.validate()?;
        Ok(page)
    }

    pub async fn viewport(
        &self,
        start: usize,
        count: usize,
        column_start: usize,
        column_count: usize,
    ) -> Result<ViewportPage, OutputError> {
        if count > crate::result_store::MAX_PAGE_ROWS
            || column_count == 0
            || column_count > MAX_VIEW_COLUMNS
        {
            return Err(OutputError::InvalidPage(
                "Viewport exceeds the row or column limit",
            ));
        }
        let page = self.page(start, count).await?;
        if column_start >= page.columns.len() || column_count > page.columns.len() - column_start {
            return Err(OutputError::InvalidPage(
                "Viewport columns are outside the schema",
            ));
        }
        Ok(ViewportPage {
            document: self.scope.document.0,
            generation: self.scope.generation,
            start,
            column_start,
            rows: page
                .rows
                .iter()
                .map(|row| {
                    row.iter()
                        .skip(column_start)
                        .take(column_count)
                        .map(CellPreview::from)
                        .collect()
                })
                .collect(),
        })
    }

    pub async fn cell(
        &self,
        row: usize,
        column: usize,
        offset: usize,
        count: usize,
    ) -> Result<CellChunk, OutputError> {
        if count > MAX_CELL_CHUNK_SCALARS {
            return Err(OutputError::InvalidChunk(
                "Cell chunks are limited to 65536 Unicode scalars",
            ));
        }
        let page = self.page(row, 1).await?;
        let cell = page
            .rows
            .first()
            .and_then(|row| row.get(column))
            .ok_or(OutputError::InvalidCell)?;
        let (text, total) = match cell {
            ResultCell::Null => (None, 0),
            ResultCell::Text(text) => (
                Some(text.chars().skip(offset).take(count).collect()),
                text.chars().count(),
            ),
        };
        if offset > total {
            return Err(OutputError::InvalidChunk(
                "Cell offset is outside the value",
            ));
        }
        Ok(CellChunk {
            document: self.scope.document.0,
            generation: self.scope.generation,
            row,
            column,
            offset,
            total,
            text,
        })
    }

    pub async fn original_cell(
        &self,
        row: usize,
        column: usize,
    ) -> Result<ResultCell, OutputError> {
        let page = self.page(row, 1).await?;
        let cell = page
            .rows
            .first()
            .and_then(|row| row.get(column))
            .ok_or(OutputError::InvalidCell)?;
        if matches!(cell, ResultCell::Text(text) if text.len() > ORIGINAL_CELL_BYTES) {
            return Err(OutputError::CellTooLarge);
        }
        Ok(cell.clone())
    }
}
