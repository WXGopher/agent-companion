//! Native desktop toasts, with Agent Companion's own Start-menu identity.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::mpsc::{RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PVCHF_DEFAULT, PropVariantChangeType,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile,
};
use windows::Win32::System::Variant::VT_LPWSTR;
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{
    FOLDERID_Programs, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath, ShellLink,
};
use windows::core::{GUID, HSTRING, Interface, PCWSTR};

use super::{Attention, Completion, WaitingUpdate};

const APP_ID: &str = "Atoll.Desktop";
const POPUP_DURATION: Duration = Duration::from_secs(3);
const APP_ID_KEY: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};

pub struct Notifier {
    tx: Sender<Delivery>,
}

enum Delivery {
    Completion(Completion),
    Waiting(WaitingUpdate),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Identity {
    tag: String,
    group: &'static str,
}

impl Notifier {
    pub fn new() -> Self {
        // Clear commands must not be dropped during a burst of concurrent
        // requests: otherwise an answered request remains in Action Center.
        let (tx, rx) = channel::<Delivery>();
        let spawned = std::thread::Builder::new()
            .name("agent-companion-notifications".into())
            .spawn(move || {
                if let Err(error) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
                    crate::util::debug_log(&format!("notifications unavailable: {error}"));
                    return;
                }
                // Waiting callbacks belong to this process. Discard previous
                // runs' inert waiting toasts while preserving completion history.
                if let Err(error) = ToastNotificationManager::History().and_then(|history| {
                    history.RemoveGroupWithId(&HSTRING::from("attention"), &HSTRING::from(APP_ID))
                }) {
                    crate::util::debug_log(&format!(
                        "waiting notification cleanup failed: {error}"
                    ));
                }
                let mut ready = false;
                let mut active = VecDeque::new();
                // Keep every outstanding request's activation handler alive,
                // independently of the bounded completion history.
                let mut waiting = HashMap::new();
                let mut popups: VecDeque<(Instant, Identity, ToastNotification)> = VecDeque::new();
                loop {
                    // Expire before receiving more work so a busy stream cannot
                    // keep an earlier popup on screen beyond its own deadline.
                    hide_expired(&mut popups);
                    let received = match popups.front() {
                        Some((deadline, _, _)) => {
                            rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        }
                        None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                    };
                    let delivery = match received {
                        Ok(delivery) => delivery,
                        Err(RecvTimeoutError::Timeout) => continue,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    let result = (|| {
                        if let Delivery::Waiting(WaitingUpdate::Clear {
                            session_id,
                            request_key,
                        }) = &delivery
                        {
                            let identity = waiting_identity(session_id, request_key);
                            popups.retain(|(_, earlier, _)| earlier != &identity);
                            if let Some(toast) = waiting.remove(&identity) {
                                hide(&toast);
                            }
                            remove(&identity)?;
                            return Ok(());
                        }
                        if !ready {
                            let exe = agent_companion_core::install::stable_bin_dir()
                                .ok()
                                .map(|dir| dir.join("agent-companion.exe"))
                                .filter(|path| path.is_file())
                                .or_else(|| std::env::current_exe().ok())
                                .ok_or_else(windows::core::Error::from_win32)?;
                            register_shortcut(&exe)?;
                            ready = true;
                        }
                        let (identity, toast) = match &delivery {
                            Delivery::Completion(completion) => (
                                completion_identity(&completion.session_id),
                                show(completion)?,
                            ),
                            Delivery::Waiting(WaitingUpdate::Show(attention)) => (
                                waiting_identity(&attention.session_id, &attention.request_key),
                                show_waiting(attention)?,
                            ),
                            Delivery::Waiting(WaitingUpdate::Clear { .. }) => unreachable!(),
                        };
                        // Only replace this exact notification's timer. A
                        // completion and a question for one session coexist.
                        popups.retain(|(_, earlier, _)| earlier != &identity);
                        popups.push_back((
                            Instant::now() + POPUP_DURATION,
                            identity.clone(),
                            toast.clone(),
                        ));
                        if identity.group == "attention" {
                            waiting.insert(identity, toast);
                        } else {
                            active.push_back(toast);
                            while active.len() > 16 {
                                active.pop_front();
                            }
                        }
                        Ok::<_, windows::core::Error>(())
                    })();
                    if let Err(error) = result {
                        crate::util::debug_log(&format!("notification failed: {error}"));
                    }
                }
                for (_, _, toast) in popups {
                    hide(&toast);
                }
                // Once this worker exits, no retained activation handler can
                // route a waiting toast. Completion history keeps its existing
                // shutdown behavior.
                for identity in waiting.keys() {
                    let _ = remove(identity);
                }
                drop(waiting);
                drop(active);
                unsafe { RoUninitialize() };
            });
        if let Err(error) = spawned {
            crate::util::debug_log(&format!("notification worker: {error}"));
        }
        Self { tx }
    }

    pub fn send(&self, completion: Completion) {
        self.queue(Delivery::Completion(completion));
    }

    pub fn send_waiting(&self, update: WaitingUpdate) {
        self.queue(Delivery::Waiting(update));
    }

    fn queue(&self, delivery: Delivery) {
        if let Err(error) = self.tx.send(delivery) {
            crate::util::debug_log(&format!("notification not queued: {error}"));
        }
    }
}

fn hide_expired(popups: &mut VecDeque<(Instant, Identity, ToastNotification)>) {
    while popups
        .front()
        .is_some_and(|(deadline, _, _)| *deadline <= Instant::now())
    {
        if let Some((_, _, toast)) = popups.pop_front() {
            hide(&toast);
        }
    }
}

fn remove(identity: &Identity) -> windows::core::Result<()> {
    ToastNotificationManager::History()?.RemoveGroupedTagWithId(
        &HSTRING::from(&identity.tag),
        &HSTRING::from(identity.group),
        &HSTRING::from(APP_ID),
    )
}

fn hide(toast: &ToastNotification) {
    // XML duration only offers "short" or "long"; explicitly withdraw the
    // popup after three seconds without changing Windows' global preferences.
    // https://learn.microsoft.com/uwp/api/windows.ui.notifications.toastnotifier.hide
    let result = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))
        .and_then(|notifier| notifier.Hide(toast));
    if let Err(error) = result {
        crate::util::debug_log(&format!("notification hide failed: {error}"));
    }
}

fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    text.encode_wide().chain(Some(0)).collect()
}

fn register_shortcut(exe: &Path) -> windows::core::Result<()> {
    unsafe {
        let programs = SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, None)?;
        let path = programs.to_string();
        CoTaskMemFree(Some(programs.0.cast()));
        let path = std::path::PathBuf::from(path?).join("Atoll.lnk");
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(PCWSTR(wide(exe.as_os_str()).as_ptr()))?;
        if let Some(parent) = exe.parent() {
            link.SetWorkingDirectory(PCWSTR(wide(parent.as_os_str()).as_ptr()))?;
        }
        link.SetDescription(windows::core::w!(
            "Agent Companion agent sessions and usage"
        ))?;
        let properties: IPropertyStore = link.cast()?;
        let mut app_id = PROPVARIANT::default();
        PropVariantChangeType(
            &mut app_id,
            &PROPVARIANT::from(APP_ID),
            PVCHF_DEFAULT,
            VT_LPWSTR,
        )?;
        properties.SetValue(&APP_ID_KEY, &app_id)?;
        properties.Commit()?;
        let file: IPersistFile = link.cast()?;
        file.Save(PCWSTR(wide(path.as_os_str()).as_ptr()), true)?;
    }
    Ok(())
}

fn show(completion: &Completion) -> windows::core::Result<ToastNotification> {
    let identity = completion_identity(&completion.session_id);
    let toast = create(&completion.title, &completion.body, &identity)?;
    let session = completion.session_id.clone();
    toast.Activated(&TypedEventHandler::new(move |_, _| {
        let session = session.clone();
        let _ = slint::invoke_from_event_loop(move || super::super::notification_clicked(&session));
        Ok(())
    }))?;
    publish(&toast)?;
    Ok(toast)
}

fn show_waiting(attention: &Attention) -> windows::core::Result<ToastNotification> {
    let identity = waiting_identity(&attention.session_id, &attention.request_key);
    let toast = create(&attention.title, &attention.body, &identity)?;
    let session = attention.session_id.clone();
    let request = attention.request_key.clone();
    toast.Activated(&TypedEventHandler::new(move |_, _| {
        let session = session.clone();
        let request = request.clone();
        let _ = slint::invoke_from_event_loop(move || {
            super::super::waiting_notification_clicked(&session, &request)
        });
        Ok(())
    }))?;
    publish(&toast)?;
    Ok(toast)
}

fn completion_identity(session_id: &str) -> Identity {
    Identity {
        tag: tag(&session_id),
        group: "completion",
    }
}

fn waiting_identity(session_id: &str, request_key: &str) -> Identity {
    Identity {
        tag: tag(&(session_id, request_key)),
        group: "attention",
    }
}

fn tag(value: &impl std::hash::Hash) -> String {
    use std::hash::Hasher;
    let mut hash = std::hash::DefaultHasher::new();
    value.hash(&mut hash);
    format!("{:016x}", hash.finish())
}

fn create(
    title: &str,
    body: &str,
    identity: &Identity,
) -> windows::core::Result<ToastNotification> {
    let document = XmlDocument::new()?;
    document.LoadXml(&HSTRING::from(xml(title, body)))?;
    let toast = ToastNotification::CreateToastNotification(&document)?;
    toast.SetTag(&HSTRING::from(&identity.tag))?;
    toast.SetGroup(&HSTRING::from(identity.group))?;
    Ok(toast)
}

fn publish(toast: &ToastNotification) -> windows::core::Result<()> {
    toast.Failed(&TypedEventHandler::new(
        |_, args: windows::core::Ref<windows::UI::Notifications::ToastFailedEventArgs>| {
            if let Some(args) = args.as_ref() {
                crate::util::debug_log(&format!(
                    "Windows rejected notification: {:?}",
                    args.ErrorCode()
                ));
            }
            Ok(())
        },
    ))?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?.Show(toast)
}

fn xml(title: &str, body: &str) -> String {
    format!(
        "<toast duration=\"short\"><visual><binding template=\"ToastText02\"><text id=\"1\">{}</text><text id=\"2\">{}</text></binding></visual><audio silent=\"true\"/></toast>",
        escape(title),
        escape(body)
    )
}

fn escape(text: &str) -> String {
    text.chars()
        .filter(|c| {
            matches!(
                c,
                '\t' | '\n' | '\r' | ' '..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}'
            )
        })
        .fold(String::new(), |mut out, c| {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&apos;"),
                _ => out.push(c),
            }
            out
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::sync_channel;

    #[test]
    #[ignore = "sends a real Windows toast; requires installed Agent Companion and enabled notifications"]
    fn native_completion_reaches_windows_notification_history() {
        unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.unwrap();
        let exe = agent_companion_core::install::stable_bin_dir()
            .unwrap()
            .join("agent-companion.exe");
        assert!(exe.is_file(), "install Agent Companion first");
        register_shortcut(&exe).unwrap();
        let app_id = HSTRING::from(APP_ID);
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&app_id).unwrap();
        let toast = show(&Completion {
            session_id: format!("agent-companion-native-test-{}", std::process::id()),
            title: "Agent Companion notification verification".into(),
            body: "Background completion delivery test.".into(),
        })
        .unwrap();
        // Before the first Show, Setting can return ERROR_NOT_FOUND because
        // Windows has not created this app's notification preferences yet.
        assert_eq!(
            notifier.Setting().unwrap(),
            windows::UI::Notifications::NotificationSetting::Enabled
        );
        let history = ToastNotificationManager::History().unwrap();
        let tag = toast.Tag().unwrap();
        let group = toast.Group().unwrap();
        let mut arrived = false;
        for _ in 0..20 {
            arrived = history
                .GetHistoryWithId(&app_id)
                .unwrap()
                .into_iter()
                .any(|item| item.Tag().ok().as_ref() == Some(&tag));
            if arrived {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        // Exercise the real Windows withdrawal path, including its dismissal
        // callback, rather than assuming XML's "short" means three seconds.
        let (dismissed_tx, dismissed_rx) = sync_channel(1);
        toast
            .Dismissed(&TypedEventHandler::new(
                move |_,
                      args: windows::core::Ref<
                    windows::UI::Notifications::ToastDismissedEventArgs,
                >| {
                    if let Some(args) = args.as_ref() {
                        let _ = dismissed_tx.try_send(args.Reason()?);
                    }
                    Ok(())
                },
            ))
            .unwrap();
        let mut popups = VecDeque::from([(
            Instant::now() + POPUP_DURATION,
            completion_identity("native-test"),
            toast.clone(),
        )]);
        assert!(matches!(
            dismissed_rx.recv_timeout(POPUP_DURATION),
            Err(RecvTimeoutError::Timeout)
        ));
        hide_expired(&mut popups);
        assert!(popups.is_empty());
        let dismissal = dismissed_rx.recv_timeout(Duration::from_secs(2));
        history
            .RemoveGroupedTagWithId(&tag, &group, &app_id)
            .unwrap();
        drop((history, toast, notifier));
        unsafe { RoUninitialize() };
        assert!(
            arrived,
            "Windows did not retain the completion notification"
        );
        assert_eq!(
            dismissal.unwrap(),
            windows::UI::Notifications::ToastDismissalReason::ApplicationHidden
        );
    }

    #[test]
    fn project_names_cannot_inject_toast_markup() {
        let document = xml("<project>&\u{0}", "done \"now\"");
        assert!(document.contains("&lt;project&gt;&amp;"));
        assert!(!document.contains('\u{0}'));
        assert!(document.contains("done &quot;now&quot;"));
    }

    #[test]
    fn waiting_toasts_are_silent_short_and_escape_question_text() {
        let document = xml(
            "Codex · <demo>",
            "Use \"fast\" & 'safe'?\u{b}\u{fffe}\u{ffff} 🦀",
        );
        assert!(document.contains("<toast duration=\"short\">"));
        assert!(document.contains("<audio silent=\"true\"/>"));
        assert!(document.contains("&lt;demo&gt;"));
        assert!(document.contains("&quot;fast&quot; &amp; &apos;safe&apos;?"));
        assert!(document.contains('🦀'));
        for invalid in ['\u{b}', '\u{fffe}', '\u{ffff}'] {
            assert!(!document.contains(invalid));
        }
        assert_eq!(POPUP_DURATION, Duration::from_secs(3));
    }

    #[test]
    fn toast_identity_is_scoped_to_kind_session_and_request() {
        let first = waiting_identity("session", "question:a");
        assert_eq!(first, waiting_identity("session", "question:a"));
        assert_ne!(first, waiting_identity("session", "question:b"));
        assert_ne!(first, waiting_identity("other-session", "question:a"));
        assert_ne!(first, completion_identity("session"));
        assert_eq!(first.group, "attention");
        assert_eq!(completion_identity("session").group, "completion");
        assert_eq!(first.tag.len(), 16);
        assert_ne!(waiting_identity("ab", "c"), waiting_identity("a", "bc"));
    }
}
