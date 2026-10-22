//! The display picker keeps the exact mapping the user is interacting with.
//! Display names and Windows display numbers can be reused; only the backend's
//! stable identity is a preference worth saving.

use super::win::TaskbarDisplay;
use slint::{ModelRc, VecModel};

#[derive(Debug, PartialEq, Eq)]
struct Options {
    labels: Vec<String>,
    targets: Vec<Option<String>>,
}

impl Options {
    fn new(displays: &[TaskbarDisplay], preferred: Option<&str>) -> Self {
        let mut options = Self {
            labels: vec!["Follow primary display".into()],
            targets: vec![None],
        };
        for display in displays {
            let suffix = match (display.primary, display.has_taskbar) {
                (true, true) => " (primary)",
                (true, false) => " (primary, no taskbar)",
                (false, true) => "",
                (false, false) => " (no taskbar)",
            };
            options
                .labels
                .push(format!("{}{suffix}", display_name(display)));
            options.targets.push(Some(display.id.clone()));
        }
        if let Some(preferred) = preferred
            && !displays
                .iter()
                .any(|display| display.id.eq_ignore_ascii_case(preferred))
        {
            options
                .labels
                .push("Selected display (disconnected)".into());
            options.targets.push(Some(preferred.into()));
        }
        options
    }
}

#[derive(Debug, Default)]
pub(super) struct DisplaySettings {
    options: Option<Options>,
}

impl DisplaySettings {
    pub fn refresh_window(
        &mut self,
        window: &super::ui::CodexTuiWindow,
        displays: &[TaskbarDisplay],
        preferred: Option<&str>,
        enabled: bool,
        force: bool,
    ) {
        let selecting =
            !force && window.get_settings_page() == 1 && window.get_taskbar_display_selecting();
        if self.refresh(displays, preferred, selecting) {
            window.set_taskbar_displays(ModelRc::new(VecModel::from(
                self.labels()
                    .iter()
                    .map(|label| label.as_str().into())
                    .collect::<Vec<_>>(),
            )));
        }
        window.set_taskbar_display_index(self.selected_index(preferred));
        window.set_taskbar_display_status(status(displays, preferred, enabled).into());
    }

    /// Do not replace the mapping while the ComboBox or its popup has focus.
    /// A display can be unplugged between a pointer press and release, and the
    /// same visible label can already belong to a different stable identity.
    /// Retaining the snapshot saves the display the user actually chose.
    pub fn refresh(
        &mut self,
        displays: &[TaskbarDisplay],
        preferred: Option<&str>,
        selecting: bool,
    ) -> bool {
        if selecting && self.options.is_some() {
            return false;
        }
        let next = Options::new(displays, preferred);
        if self.options.as_ref() == Some(&next) {
            return false;
        }
        self.options = Some(next);
        true
    }

    pub fn labels(&self) -> &[String] {
        self.options.as_ref().map_or(&[], |options| &options.labels)
    }

    pub fn selected_index(&self, preferred: Option<&str>) -> i32 {
        self.options
            .as_ref()
            .and_then(|options| {
                options
                    .targets
                    .iter()
                    .position(|target| match (target.as_deref(), preferred) {
                        (None, None) => true,
                        (Some(target), Some(preferred)) => target.eq_ignore_ascii_case(preferred),
                        _ => false,
                    })
            })
            .unwrap_or(0) as i32
    }

    /// `None` rejects an invalid row; `Some(None)` explicitly follows primary.
    pub fn selection(&self, index: i32) -> Option<Option<String>> {
        let index = usize::try_from(index).ok()?;
        self.options.as_ref()?.targets.get(index).cloned()
    }
}

pub(super) fn status(
    displays: &[TaskbarDisplay],
    preferred: Option<&str>,
    enabled: bool,
) -> String {
    let primary_available = displays
        .iter()
        .any(|display| display.primary && display.has_taskbar);
    let fallback = if primary_available {
        "Using the primary display"
    } else {
        "Waiting for a taskbar on the primary display"
    };
    let mut status = match preferred {
        None if primary_available => "Follows the primary display.".into(),
        None => "Waiting for a taskbar on the primary display.".into(),
        Some(preferred) => match displays
            .iter()
            .find(|display| display.id.eq_ignore_ascii_case(preferred))
        {
            Some(display) if display.has_taskbar => format!("Uses {}.", display_name(display)),
            Some(display) => format!(
                "No taskbar on {}. {fallback} until its taskbar is available.",
                display_name(display)
            ),
            None => {
                format!("The selected display is disconnected. {fallback} until it reconnects.")
            }
        },
    };
    if !enabled {
        status.push_str(" The readout is hidden; this choice is saved.");
    }
    status
}

fn display_name(display: &TaskbarDisplay) -> &str {
    let name = display.name.trim();
    if name.is_empty() { "Display" } else { name }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(id: &str, name: &str, primary: bool, has_taskbar: bool) -> TaskbarDisplay {
        TaskbarDisplay {
            id: id.into(),
            name: name.into(),
            primary,
            has_taskbar,
        }
    }

    #[test]
    fn lists_every_display_and_keeps_primary_following_separate_from_a_fixed_primary() {
        let displays = [
            display("main", "Display 1 · Desk", true, true),
            display("side", "Display 2 · Side", false, false),
        ];
        let mut settings = DisplaySettings::default();
        assert!(settings.refresh(&displays, None, false));
        assert_eq!(
            settings.labels(),
            [
                "Follow primary display",
                "Display 1 · Desk (primary)",
                "Display 2 · Side (no taskbar)",
            ]
        );
        assert_eq!(settings.selected_index(None), 0);
        assert_eq!(settings.selected_index(Some("main")), 1);
        assert_eq!(settings.selection(0), Some(None));
        assert_eq!(settings.selection(1), Some(Some("main".into())));
        assert_eq!(settings.selection(-1), None);
        assert_eq!(settings.selection(99), None);
        assert!(!settings.refresh(&displays, None, false));
    }

    #[test]
    fn disconnected_preferences_remain_selected_and_reconnect_by_identity() {
        let main = display("main", "Display 1", true, true);
        let side = display("side", "Display 2", false, true);
        let mut settings = DisplaySettings::default();
        settings.refresh(std::slice::from_ref(&main), Some("side"), false);
        assert_eq!(settings.selected_index(Some("side")), 2);
        assert_eq!(settings.labels()[2], "Selected display (disconnected)");
        assert_eq!(settings.selection(2), Some(Some("side".into())));
        assert!(
            status(std::slice::from_ref(&main), Some("side"), true).contains("until it reconnects")
        );

        assert!(settings.refresh(&[main, side], Some("side"), false));
        assert_eq!(settings.selected_index(Some("side")), 2);
        assert_eq!(settings.labels()[2], "Display 2");
    }

    #[test]
    fn open_picker_retains_ids_when_labels_and_display_numbers_are_reused() {
        let original = [
            display("first-device", "Display 1 · Same model", true, true),
            display("second-device", "Display 2 · Same model", false, true),
        ];
        let changed = [
            display("replacement-device", "Display 1 · Same model", true, true),
            display("another-device", "Display 2 · Same model", false, true),
        ];
        let mut settings = DisplaySettings::default();
        settings.refresh(&original, None, false);
        assert!(!settings.refresh(&changed, None, true));
        assert_eq!(settings.selection(1), Some(Some("first-device".into())));
        assert_eq!(settings.selection(2), Some(Some("second-device".into())));

        assert!(settings.refresh(&changed, Some("second-device"), false));
        assert_eq!(settings.selected_index(Some("second-device")), 3);
        assert_eq!(settings.selection(3), Some(Some("second-device".into())));
        assert_eq!(settings.selection(2), Some(Some("another-device".into())));
    }

    #[test]
    fn identical_names_never_substitute_for_a_stable_identity() {
        let displays = [
            display("one", "Matching panel", false, true),
            display("two", "Matching panel", false, true),
        ];
        let mut settings = DisplaySettings::default();
        settings.refresh(&displays, Some("two"), false);
        assert_eq!(settings.labels()[1], settings.labels()[2]);
        assert_eq!(settings.selected_index(Some("two")), 2);
        assert_eq!(settings.selection(1), Some(Some("one".into())));
        assert_eq!(settings.selection(2), Some(Some("two".into())));
    }

    #[test]
    fn saved_identity_matching_is_case_insensitive_like_windows() {
        let displays = [display(r"\\?\DISPLAY#ABC#Stable", "Display 1", true, true)];
        let preferred = r"\\?\display#abc#stable";
        let mut settings = DisplaySettings::default();
        settings.refresh(&displays, Some(preferred), false);
        assert_eq!(
            settings.labels().len(),
            2,
            "connected IDs must not appear disconnected"
        );
        assert_eq!(settings.selected_index(Some(preferred)), 1);
        assert_eq!(status(&displays, Some(preferred), true), "Uses Display 1.");
    }

    #[test]
    fn status_explains_temporary_fallback_and_hidden_readout() {
        let displays = [
            display("main", "Display 1", true, true),
            display("side", "Display 2", false, false),
        ];
        let text = status(&displays, Some("side"), false);
        assert!(text.contains("No taskbar on Display 2"));
        assert!(text.contains("Using the primary display until its taskbar is available"));
        assert!(text.contains("The readout is hidden; this choice is saved"));
        assert_eq!(
            status(&displays, None, true),
            "Follows the primary display."
        );
        assert!(status(&[], Some("side"), true).contains("Waiting for a taskbar"));
    }
}
