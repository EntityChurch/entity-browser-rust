//! What the Files renderer draws — plain data, no DOM.

use crate::file_kinds::{FileRow, Place};

/// The last thing a press did, beside the list it acted on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesNotice {
    Info(String),
    Error(String),
}

#[derive(Debug, Clone)]
pub struct FilesOutput {
    pub peer_id: String,
    pub place: Place,
    /// Every place with how many files it holds, in [`Place::ALL`] order.
    pub counts: Vec<(Place, usize)>,
    /// The files in the selected place.
    pub rows: Vec<FileRow>,
    pub notice: Option<FilesNotice>,
}
