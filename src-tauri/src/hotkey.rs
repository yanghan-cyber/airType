use rdev::{Key, EventType};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub enum HotkeyState {
    Idle,
    Pressed(Instant),
    Held(Instant),
    Released,
}

#[derive(Debug, Clone)]
pub struct HotkeyConfig {
    pub modifiers: Vec<Key>,
    pub main_key: Key,
    pub debounce_ms: u64,
    pub max_hold_secs: u64,
}

impl HotkeyConfig {
    pub fn default_ctrl_win() -> Self {
        Self {
            modifiers: vec![Key::ControlLeft],
            main_key: Key::MetaLeft,
            debounce_ms: 100,
            max_hold_secs: 30,
        }
    }

    pub fn from_string(s: &str) -> Result<Self, String> {
        let parts: Vec<&str> = s.split('+').collect();
        if parts.is_empty() || parts.iter().all(|p| p.trim().is_empty()) {
            return Err("Hotkey cannot be empty".to_string());
        }
        if parts.len() == 1 {
            // Single key hotkey
            let main_key = parse_key_name(parts[0])?;
            Ok(Self {
                modifiers: vec![],
                main_key,
                debounce_ms: 100,
                max_hold_secs: 30,
            })
        } else {
            let mut modifiers = Vec::new();
            for part in &parts[..parts.len() - 1] {
                modifiers.push(parse_key_name(part)?);
            }
            let main_key = parse_key_name(parts.last().unwrap())?;
            Ok(Self {
                modifiers,
                main_key,
                debounce_ms: 100,
                max_hold_secs: 30,
            })
        }
    }
}

fn parse_key_name(name: &str) -> Result<Key, String> {
    match name {
        // Left-side modifiers
        "LCtrl" | "Ctrl" => Ok(Key::ControlLeft),
        "LAlt" => Ok(Key::Alt),
        "LShift" => Ok(Key::ShiftLeft),
        "LWin" | "LMeta" => Ok(Key::MetaLeft),
        // Right-side modifiers
        "RCtrl" => Ok(Key::ControlRight),
        "RAlt" => Ok(Key::AltGr),
        "RShift" => Ok(Key::ShiftRight),
        "RWin" | "RMeta" => Ok(Key::MetaRight),
        // Generic modifiers (map to left)
        "Alt" => Ok(Key::Alt),
        "Shift" => Ok(Key::ShiftLeft),
        "Win" | "Meta" => Ok(Key::MetaLeft),
        "Space" => Ok(Key::Space),
        "Return" | "Enter" => Ok(Key::Return),
        "Escape" | "Esc" => Ok(Key::Escape),
        "Tab" => Ok(Key::Tab),
        "Backspace" => Ok(Key::Backspace),
        "Delete" => Ok(Key::Delete),
        "Left" => Ok(Key::LeftArrow),
        "Right" => Ok(Key::RightArrow),
        "Up" => Ok(Key::UpArrow),
        "Down" => Ok(Key::DownArrow),
        "A" => Ok(Key::KeyA), "B" => Ok(Key::KeyB), "C" => Ok(Key::KeyC),
        "D" => Ok(Key::KeyD), "E" => Ok(Key::KeyE), "F" => Ok(Key::KeyF),
        "G" => Ok(Key::KeyG), "H" => Ok(Key::KeyH), "I" => Ok(Key::KeyI),
        "J" => Ok(Key::KeyJ), "K" => Ok(Key::KeyK), "L" => Ok(Key::KeyL),
        "M" => Ok(Key::KeyM), "N" => Ok(Key::KeyN), "O" => Ok(Key::KeyO),
        "P" => Ok(Key::KeyP), "Q" => Ok(Key::KeyQ), "R" => Ok(Key::KeyR),
        "S" => Ok(Key::KeyS), "T" => Ok(Key::KeyT), "U" => Ok(Key::KeyU),
        "V" => Ok(Key::KeyV), "W" => Ok(Key::KeyW), "X" => Ok(Key::KeyX),
        "Y" => Ok(Key::KeyY), "Z" => Ok(Key::KeyZ),
        "0" => Ok(Key::Num0), "1" => Ok(Key::Num1), "2" => Ok(Key::Num2),
        "3" => Ok(Key::Num3), "4" => Ok(Key::Num4), "5" => Ok(Key::Num5),
        "6" => Ok(Key::Num6), "7" => Ok(Key::Num7), "8" => Ok(Key::Num8),
        "9" => Ok(Key::Num9),
        "F1" => Ok(Key::F1), "F2" => Ok(Key::F2), "F3" => Ok(Key::F3),
        "F4" => Ok(Key::F4), "F5" => Ok(Key::F5), "F6" => Ok(Key::F6),
        "F7" => Ok(Key::F7), "F8" => Ok(Key::F8), "F9" => Ok(Key::F9),
        "F10" => Ok(Key::F10), "F11" => Ok(Key::F11), "F12" => Ok(Key::F12),
        _ => Err(format!("Unknown key: {}", name)),
    }
}

/// Tracker for modifier key states.
pub struct KeyStateTracker {
    pressed_keys: std::collections::HashSet<Key>,
    state: HotkeyState,
    config: HotkeyConfig,
}

impl KeyStateTracker {
    pub fn new(config: HotkeyConfig) -> Self {
        Self {
            pressed_keys: std::collections::HashSet::new(),
            state: HotkeyState::Idle,
            config,
        }
    }

    pub fn process_event(&mut self, event: &EventType) -> Option<HotkeyTransition> {
        match event {
            EventType::KeyPress(key) => {
                self.pressed_keys.insert(*key);
                match &self.state {
                    HotkeyState::Idle => {
                        if self.is_hotkey_pressed() {
                            self.state = HotkeyState::Pressed(Instant::now());
                            Some(HotkeyTransition::Pressed)
                        } else {
                            None
                        }
                    }
                    _ => None,
                }
            }
            EventType::KeyRelease(key) => {
                self.pressed_keys.remove(key);
                match &self.state {
                    HotkeyState::Pressed(instant) => {
                        let elapsed = instant.elapsed();
                        self.state = HotkeyState::Idle;
                        self.pressed_keys.clear();
                        if elapsed < Duration::from_millis(self.config.debounce_ms) {
                            Some(HotkeyTransition::Debounced)
                        } else {
                            Some(HotkeyTransition::ReleasedTooEarly)
                        }
                    }
                    HotkeyState::Held(_) => {
                        self.state = HotkeyState::Idle;
                        self.pressed_keys.clear();
                        Some(HotkeyTransition::Released)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    pub fn tick(&mut self) -> Option<HotkeyTransition> {
        match &self.state {
            HotkeyState::Pressed(instant) => {
                if instant.elapsed() >= Duration::from_millis(self.config.debounce_ms) {
                    let i = *instant;
                    self.state = HotkeyState::Held(i);
                    Some(HotkeyTransition::Confirmed)
                } else {
                    None
                }
            }
            HotkeyState::Held(instant) => {
                if instant.elapsed() >= Duration::from_secs(self.config.max_hold_secs) {
                    self.state = HotkeyState::Idle;
                    self.pressed_keys.clear();
                    Some(HotkeyTransition::Timeout)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn is_hotkey_pressed(&self) -> bool {
        let all_mods = self.config.modifiers.iter().all(|m| self.pressed_keys.contains(m));
        let has_main = self.pressed_keys.contains(&self.config.main_key);
        all_mods && has_main
    }

    pub fn update_config(&mut self, config: HotkeyConfig) {
        self.config = config;
    }

    pub fn state(&self) -> HotkeyState {
        self.state.clone()
    }
}

#[derive(Debug, PartialEq)]
pub enum HotkeyTransition {
    Pressed,
    Confirmed,
    Released,
    Debounced,
    ReleasedTooEarly,
    Timeout,
}

pub struct DualHotkeyTracker {
    primary: KeyStateTracker,
    secondary: Option<KeyStateTracker>,
}

#[derive(Debug, PartialEq)]
pub enum HotkeySource {
    Primary,
    Secondary,
}

impl DualHotkeyTracker {
    pub fn new(primary_config: HotkeyConfig, secondary_config: Option<HotkeyConfig>) -> Self {
        Self {
            primary: KeyStateTracker::new(primary_config),
            secondary: secondary_config.map(KeyStateTracker::new),
        }
    }

    pub fn process_event(&mut self, event: &EventType) -> Option<(HotkeySource, HotkeyTransition)> {
        if let Some(transition) = self.primary.process_event(event) {
            return Some((HotkeySource::Primary, transition));
        }
        if let Some(ref mut secondary) = self.secondary {
            if let Some(transition) = secondary.process_event(event) {
                return Some((HotkeySource::Secondary, transition));
            }
        }
        None
    }

    pub fn tick(&mut self) -> Option<(HotkeySource, HotkeyTransition)> {
        if let Some(transition) = self.primary.tick() {
            return Some((HotkeySource::Primary, transition));
        }
        if let Some(ref mut secondary) = self.secondary {
            if let Some(transition) = secondary.tick() {
                return Some((HotkeySource::Secondary, transition));
            }
        }
        None
    }

    pub fn update_primary(&mut self, config: HotkeyConfig) {
        self.primary.update_config(config);
    }

    pub fn update_secondary(&mut self, config: Option<HotkeyConfig>) {
        match (&mut self.secondary, config) {
            (Some(tracker), Some(cfg)) => tracker.update_config(cfg),
            (Some(_), None) => self.secondary = None,
            (None, Some(cfg)) => self.secondary = Some(KeyStateTracker::new(cfg)),
            (None, None) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hotkey_config_default() {
        let cfg = HotkeyConfig::default_ctrl_win();
        assert_eq!(cfg.modifiers, vec![Key::ControlLeft]);
        assert_eq!(cfg.main_key, Key::MetaLeft);
        assert_eq!(cfg.debounce_ms, 100);
        assert_eq!(cfg.max_hold_secs, 30);
    }

    #[test]
    fn test_hotkey_config_from_string() {
        let cfg = HotkeyConfig::from_string("Ctrl+Win").unwrap();
        assert_eq!(cfg.modifiers, vec![Key::ControlLeft]);
        assert_eq!(cfg.main_key, Key::MetaLeft);
    }

    #[test]
    fn test_hotkey_config_from_string_multi() {
        let cfg = HotkeyConfig::from_string("Ctrl+Alt+Space").unwrap();
        assert_eq!(cfg.modifiers, vec![Key::ControlLeft, Key::Alt]);
        assert_eq!(cfg.main_key, Key::Space);
    }

    #[test]
    fn test_hotkey_config_from_string_single_key() {
        let cfg = HotkeyConfig::from_string("F6").unwrap();
        assert!(cfg.modifiers.is_empty());
        assert_eq!(cfg.main_key, Key::F6);
    }

    #[test]
    fn test_hotkey_config_from_string_single_key_rejected() {
        // Empty string should still be rejected
        assert!(HotkeyConfig::from_string("").is_err());
    }

    #[test]
    fn test_hotkey_config_from_string_empty_rejected() {
        assert!(HotkeyConfig::from_string("").is_err());
    }

    #[test]
    fn test_parse_key_left_right_modifiers() {
        assert_eq!(parse_key_name("LCtrl").unwrap(), Key::ControlLeft);
        assert_eq!(parse_key_name("RCtrl").unwrap(), Key::ControlRight);
        assert_eq!(parse_key_name("LShift").unwrap(), Key::ShiftLeft);
        assert_eq!(parse_key_name("RShift").unwrap(), Key::ShiftRight);
        assert_eq!(parse_key_name("LWin").unwrap(), Key::MetaLeft);
        assert_eq!(parse_key_name("RWin").unwrap(), Key::MetaRight);
    }

    #[test]
    fn test_hotkey_config_left_right_modifier_combo() {
        let cfg = HotkeyConfig::from_string("RCtrl+LShift+A").unwrap();
        assert_eq!(cfg.modifiers, vec![Key::ControlRight, Key::ShiftLeft]);
        assert_eq!(cfg.main_key, Key::KeyA);
    }

    #[test]
    fn test_tracker_press_confirm_release() {
        let cfg = HotkeyConfig::default_ctrl_win();
        let mut tracker = KeyStateTracker::new(cfg);

        assert!(tracker.process_event(&EventType::KeyPress(Key::ControlLeft)).is_none());
        assert_eq!(tracker.process_event(&EventType::KeyPress(Key::MetaLeft)),
            Some(HotkeyTransition::Pressed));

        std::thread::sleep(Duration::from_millis(210));
        assert_eq!(tracker.tick(), Some(HotkeyTransition::Confirmed));

        assert_eq!(tracker.process_event(&EventType::KeyRelease(Key::MetaLeft)),
            Some(HotkeyTransition::Released));
    }

    #[test]
    fn test_tracker_debounce_short_press() {
        let cfg = HotkeyConfig {
            debounce_ms: 100,
            ..HotkeyConfig::default_ctrl_win()
        };
        let mut tracker = KeyStateTracker::new(cfg);

        tracker.process_event(&EventType::KeyPress(Key::ControlLeft));
        tracker.process_event(&EventType::KeyPress(Key::MetaLeft));

        let result = tracker.process_event(&EventType::KeyRelease(Key::MetaLeft));
        assert_eq!(result, Some(HotkeyTransition::Debounced));
    }

    #[test]
    fn test_tracker_unrelated_keys_ignored() {
        let mut tracker = KeyStateTracker::new(HotkeyConfig::default_ctrl_win());
        assert!(tracker.process_event(&EventType::KeyPress(Key::KeyA)).is_none());
        assert!(tracker.process_event(&EventType::KeyPress(Key::KeyB)).is_none());
        assert!(tracker.process_event(&EventType::KeyRelease(Key::KeyA)).is_none());
    }

    #[test]
    fn test_tracker_tick_when_idle() {
        let mut tracker = KeyStateTracker::new(HotkeyConfig::default_ctrl_win());
        assert!(tracker.tick().is_none());
    }

    #[test]
    fn test_tracker_state_transitions() {
        let mut tracker = KeyStateTracker::new(HotkeyConfig::default_ctrl_win());
        assert!(matches!(tracker.state(), HotkeyState::Idle));

        tracker.process_event(&EventType::KeyPress(Key::ControlLeft));
        tracker.process_event(&EventType::KeyPress(Key::MetaLeft));
        assert!(matches!(tracker.state(), HotkeyState::Pressed(_)));

        std::thread::sleep(Duration::from_millis(210));
        tracker.tick();
        assert!(matches!(tracker.state(), HotkeyState::Held(_)));

        tracker.process_event(&EventType::KeyRelease(Key::MetaLeft));
        assert!(matches!(tracker.state(), HotkeyState::Idle));
    }

    #[test]
    fn test_tracker_timeout() {
        let cfg = HotkeyConfig {
            max_hold_secs: 0,
            ..HotkeyConfig::default_ctrl_win()
        };
        let mut tracker = KeyStateTracker::new(cfg);

        tracker.process_event(&EventType::KeyPress(Key::ControlLeft));
        tracker.process_event(&EventType::KeyPress(Key::MetaLeft));

        std::thread::sleep(Duration::from_millis(210));
        tracker.tick(); // Confirms
        assert_eq!(tracker.tick(), Some(HotkeyTransition::Timeout));
    }

    #[test]
    fn test_parse_key_name_known() {
        assert_eq!(parse_key_name("Ctrl").unwrap(), Key::ControlLeft);
        assert_eq!(parse_key_name("Alt").unwrap(), Key::Alt);
        assert_eq!(parse_key_name("Shift").unwrap(), Key::ShiftLeft);
        assert_eq!(parse_key_name("Win").unwrap(), Key::MetaLeft);
        assert_eq!(parse_key_name("Space").unwrap(), Key::Space);
    }

    #[test]
    fn test_parse_key_name_unknown() {
        assert!(parse_key_name("Foo").is_err());
    }

    #[test]
    fn test_parse_key_name_all_letters() {
        for c in 'A'..='Z' {
            assert!(parse_key_name(&c.to_string()).is_ok(), "Failed for key {}", c);
        }
    }

    #[test]
    fn test_parse_key_name_digits() {
        for c in '0'..='9' {
            assert!(parse_key_name(&c.to_string()).is_ok(), "Failed for key {}", c);
        }
    }

    #[test]
    fn test_tracker_update_config() {
        let cfg = HotkeyConfig::default_ctrl_win();
        let mut tracker = KeyStateTracker::new(cfg);
        let new_cfg = HotkeyConfig::from_string("Alt+X").unwrap();
        tracker.update_config(new_cfg);
        // After update, Alt+X should be recognized, Ctrl+Win should not
        assert!(tracker.process_event(&EventType::KeyPress(Key::Alt)).is_none());
        assert_eq!(tracker.process_event(&EventType::KeyPress(Key::KeyX)),
            Some(HotkeyTransition::Pressed));
    }

    #[test]
    fn test_dual_hotkey_primary() {
        let primary = HotkeyConfig::default_ctrl_win();
        let secondary = HotkeyConfig::from_string("Ctrl+Alt").ok();
        let mut tracker = DualHotkeyTracker::new(primary, secondary);

        tracker.process_event(&EventType::KeyPress(Key::ControlLeft));
        let result = tracker.process_event(&EventType::KeyPress(Key::MetaLeft));
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, HotkeySource::Primary);
    }

    #[test]
    fn test_dual_hotkey_secondary() {
        let primary = HotkeyConfig::default_ctrl_win();
        let secondary = HotkeyConfig::from_string("Ctrl+Alt").ok();
        let mut tracker = DualHotkeyTracker::new(primary, secondary);

        tracker.process_event(&EventType::KeyPress(Key::ControlLeft));
        let result = tracker.process_event(&EventType::KeyPress(Key::Alt));
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, HotkeySource::Secondary);
    }
}
