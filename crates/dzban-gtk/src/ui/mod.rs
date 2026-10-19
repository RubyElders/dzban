mod action_search;
mod app;
mod chooser;
mod editor;
mod format;
mod history;
pub(crate) mod icons;
mod navigation;
mod plan_view;
mod results;
mod row_inspector;
mod shortcuts;
mod statusbar;
mod tab_page;
#[cfg(test)]
mod tests;
mod window;
mod workspace;

pub use window::build_ui;
