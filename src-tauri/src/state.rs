use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq)]
pub enum RecordingState {
    Idle,
    Recording { started_at: std::time::Instant },
    /// Realtime WebSocket streaming. `finalized` holds sentence-finalized text,
    /// `current_partial` the in-progress partial. Display text = concat + partial.
    RealtimeRecording {
        started_at: std::time::Instant,
        finalized: Vec<String>,
        current_partial: String,
    },
    Processing,
    LlmProcessing { text: String },
    ModeSelection { text: String },
    Done,
    Error(String),
}

pub struct AppState {
    pub recording: RecordingState,
    pub enabled: bool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            recording: RecordingState::Idle,
            enabled: true,
        }
    }
}

impl AppState {
    pub fn new() -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self::default()))
    }
}

pub fn transition_to(state: &mut RecordingState, next: RecordingState) -> Result<RecordingState, String> {
    let prev = state.clone();
    match (&prev, &next) {
        (RecordingState::Idle, RecordingState::Recording { .. }) => {}
        (RecordingState::Idle, RecordingState::RealtimeRecording { .. }) => {}
        (RecordingState::Recording { .. }, RecordingState::Processing) => {}
        (RecordingState::Recording { .. }, RecordingState::Idle) => {}
        (RecordingState::RealtimeRecording { .. }, RecordingState::Processing) => {}
        (RecordingState::RealtimeRecording { .. }, RecordingState::Idle) => {}
        (RecordingState::Processing, RecordingState::Done) => {}
        (RecordingState::Processing, RecordingState::Error(_)) => {}
        (RecordingState::Processing, RecordingState::Idle) => {}
        (RecordingState::Processing, RecordingState::LlmProcessing { .. }) => {}
        (RecordingState::Processing, RecordingState::ModeSelection { .. }) => {}
        (RecordingState::LlmProcessing { .. }, RecordingState::Done) => {}
        (RecordingState::LlmProcessing { .. }, RecordingState::Error(_)) => {}
        (RecordingState::LlmProcessing { .. }, RecordingState::Idle) => {}
        (RecordingState::ModeSelection { .. }, RecordingState::LlmProcessing { .. }) => {}
        (RecordingState::ModeSelection { .. }, RecordingState::Done) => {}
        (RecordingState::ModeSelection { .. }, RecordingState::Error(_)) => {}
        (RecordingState::ModeSelection { .. }, RecordingState::Idle) => {}
        (RecordingState::Done, RecordingState::Idle) => {}
        (RecordingState::Error(_), RecordingState::Idle) => {}
        _ => return Err(format!("Invalid transition: {:?} → {:?}", prev, next)),
    }
    *state = next;
    Ok(prev)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_transitions() {
        let mut state = RecordingState::Idle;
        assert!(transition_to(&mut state, RecordingState::Recording { started_at: std::time::Instant::now() }).is_ok());
        assert!(transition_to(&mut state, RecordingState::Processing).is_ok());
        assert!(transition_to(&mut state, RecordingState::Done).is_ok());
        assert!(transition_to(&mut state, RecordingState::Idle).is_ok());
    }

    #[test]
    fn test_error_transition() {
        let mut state = RecordingState::Idle;
        transition_to(&mut state, RecordingState::Recording { started_at: std::time::Instant::now() }).unwrap();
        transition_to(&mut state, RecordingState::Processing).unwrap();
        assert!(transition_to(&mut state, RecordingState::Error("backend failed".into())).is_ok());
        assert!(transition_to(&mut state, RecordingState::Idle).is_ok());
    }

    #[test]
    fn test_recording_cancelled() {
        let mut state = RecordingState::Idle;
        transition_to(&mut state, RecordingState::Recording { started_at: std::time::Instant::now() }).unwrap();
        assert!(transition_to(&mut state, RecordingState::Idle).is_ok());
    }

    #[test]
    fn test_invalid_idle_to_processing() {
        let mut state = RecordingState::Idle;
        assert!(transition_to(&mut state, RecordingState::Processing).is_err());
    }

    #[test]
    fn test_invalid_idle_to_done() {
        let mut state = RecordingState::Idle;
        assert!(transition_to(&mut state, RecordingState::Done).is_err());
    }

    #[test]
    fn test_invalid_processing_to_recording() {
        let mut state = RecordingState::Idle;
        transition_to(&mut state, RecordingState::Recording { started_at: std::time::Instant::now() }).unwrap();
        transition_to(&mut state, RecordingState::Processing).unwrap();
        assert!(transition_to(&mut state, RecordingState::Recording { started_at: std::time::Instant::now() }).is_err());
    }

    #[test]
    fn test_app_state_default() {
        let state = AppState::default();
        assert!(matches!(state.recording, RecordingState::Idle));
        assert!(state.enabled);
    }

    #[test]
    fn test_app_state_shared() {
        let state = AppState::new();
        {
            let mut s = state.lock().unwrap();
            s.enabled = false;
        }
        assert!(!state.lock().unwrap().enabled);
    }

    #[test]
    fn test_llm_processing_transition() {
        let mut state = RecordingState::Idle;
        transition_to(&mut state, RecordingState::Recording { started_at: std::time::Instant::now() }).unwrap();
        transition_to(&mut state, RecordingState::Processing).unwrap();
        assert!(transition_to(&mut state, RecordingState::LlmProcessing { text: "test".into() }).is_ok());
        assert!(transition_to(&mut state, RecordingState::Done).is_ok());
    }

    #[test]
    fn test_mode_selection_transition() {
        let mut state = RecordingState::Idle;
        transition_to(&mut state, RecordingState::Recording { started_at: std::time::Instant::now() }).unwrap();
        transition_to(&mut state, RecordingState::Processing).unwrap();
        assert!(transition_to(&mut state, RecordingState::ModeSelection { text: "hello".into() }).is_ok());
        assert!(transition_to(&mut state, RecordingState::LlmProcessing { text: "test".into() }).is_ok());
    }

    #[test]
    fn test_mode_selection_cancel() {
        let mut state = RecordingState::Idle;
        transition_to(&mut state, RecordingState::Recording { started_at: std::time::Instant::now() }).unwrap();
        transition_to(&mut state, RecordingState::Processing).unwrap();
        transition_to(&mut state, RecordingState::ModeSelection { text: "hello".into() }).unwrap();
        assert!(transition_to(&mut state, RecordingState::Idle).is_ok());
    }

    #[test]
    fn test_realtime_recording_lifecycle() {
        let mut state = RecordingState::Idle;
        assert!(transition_to(&mut state, RecordingState::RealtimeRecording {
            started_at: std::time::Instant::now(),
            finalized: vec![], current_partial: String::new(),
        }).is_ok());
        assert!(transition_to(&mut state, RecordingState::Processing).is_ok());
        assert!(transition_to(&mut state, RecordingState::Done).is_ok());
    }

    #[test]
    fn test_realtime_recording_cancel() {
        let mut state = RecordingState::Idle;
        transition_to(&mut state, RecordingState::RealtimeRecording {
            started_at: std::time::Instant::now(),
            finalized: vec![], current_partial: String::new(),
        }).unwrap();
        assert!(transition_to(&mut state, RecordingState::Idle).is_ok());
    }
}
