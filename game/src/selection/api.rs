use bevy::prelude::*;
use puzzella_core::PieceId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelectionMode {
    Point,
    Rectangle,
}

#[derive(Clone, Copy, Debug)]
pub struct SelectionRequest {
    pub request_id: u64,
    /// Absolute logical coordinates in the camera render target, top-left origin.
    pub region: Rect,
    pub mode: SelectionMode,
}

#[derive(Clone, Debug)]
pub struct SelectionResult {
    pub request_id: u64,
    pub mode: SelectionMode,
    pub piece_ids: Vec<PieceId>,
    pub entities: Vec<Entity>,
    pub error: Option<String>,
}

#[derive(Resource, Default)]
pub struct PuzzleSelection {
    next_id: u64,
    pub latest: Option<SelectionRequest>,
    pub completed: Option<SelectionResult>,
    pub debug: bool,
}
impl PuzzleSelection {
    pub fn request(&mut self, region: Rect, mode: SelectionMode) -> u64 {
        self.next_id = self.next_id.checked_add(1).expect("selection ID exhausted");
        let request_id = self.next_id;
        self.latest = Some(SelectionRequest {
            request_id,
            region,
            mode,
        });
        self.completed = None;
        request_id
    }
    pub fn cancel(&mut self) {
        // Never reset next_id between sessions: late callbacks cannot alias a new request.
        self.latest = None;
        self.completed = None;
    }
    pub fn take_result(&mut self, id: u64) -> Option<SelectionResult> {
        if self.completed.as_ref().is_some_and(|r| r.request_id == id) {
            self.completed.take()
        } else {
            None
        }
    }
}
