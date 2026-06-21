//! Renderer-neutral output for the Event Log window.

#![allow(dead_code)]

use crate::views::EventEntry;

#[derive(Debug, Clone)]
pub struct EventLogOutput {
    /// All events in the global log, pre-classified into semantic
    /// categories so the renderer can color-code without re-parsing
    /// the messages.
    pub events: Vec<EventEntry>,
}
