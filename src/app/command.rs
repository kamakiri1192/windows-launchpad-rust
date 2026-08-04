//! Command execution at the app boundary.
//!
//! Side effects requested as [`super::event::AppCommand`] values (or
//! edit-mode [`EditModeCommand`][edit] values) are executed here. The update
//! and frame layers call these methods; they are the single place that touches
//! window visibility, the OS hotkey/tray adapter (via `platform_windows`), app
//! launching, and persistence stores.
//!
//! [edit]: crate::features::edit_mode::EditModeCommand
//!
//! The app shell is intentionally not a pure reducer: these are `&mut self`
//! methods that run eagerly, preserving the historical side-effect ordering
//! (hide before launch, modal dismiss without passthrough, etc.).

use std::time::Instant;

use crate::debug_log;
use crate::domain::app_registry::AppLaunchInfo;
use crate::features::edit_mode::EditModeCommand;
use crate::scroll::Phase;

use super::event::AppCommand;
use super::state::App;

impl App {
    /// Load the persisted user customization (Phase 7 launcher layout: item
    /// order, folders, hidden apps) into `launcher_state`. Called once at
    /// startup, before the first scan is ingested, so apps are placed in the
    /// user's arrangement from the first frame.
    ///
    /// Migration: if the Phase 7 `launcher_state` key is present it is used
    /// directly. Otherwise the legacy `app_order` + `hidden_ids` binary keys
    /// are read and converted via [`LauncherState::from_legacy`]. A missing or
    /// corrupt store is a no-op (state stays empty / non-customized), so a bad
    /// blob never blocks startup or wipes other settings.
    pub(crate) fn load_customization(&mut self) {
        self.settings = self.cache.get_settings();
        if let Some(state) = self.cache.get_launcher_state() {
            self.launcher_state = state;
            return;
        }
        // Legacy migration path: convert the old binary app_order + hidden_ids
        // keys into the item-based launcher state.
        let order = self.cache.get_app_order();
        let hidden = self.cache.get_hidden_ids();
        if !order.is_empty() || !hidden.is_empty() {
            self.launcher_state =
                crate::domain::launcher_state::LauncherState::from_legacy(order, hidden);
        }
    }

    /// Persist the current launcher layout so it survives across launches.
    /// Called after a drag-to-reorder, hide/unhide, or folder change. Cheap:
    /// one small JSON blob upsert. Errors are logged but never panic the UI.
    pub(crate) fn persist_launcher_state(&self) {
        if self.qa_enabled() {
            return;
        }
        if let Err(e) = self.cache.put_launcher_state(&self.launcher_state) {
            eprintln!("layout: failed to persist launcher state: {e}");
        }
    }

    /// Persist the current display order so it survives across launches. Called
    /// after a drag-to-reorder completes (and on hide). Phase 7 routes this
    /// through the unified launcher state; this method is kept as the
    /// edit-mode command target so the command boundary stays stable.
    pub(crate) fn persist_user_order(&self) {
        self.persist_launcher_state();
    }

    /// Persist the current hidden-app list. Called after a hide/unhide change.
    /// Phase 7 routes this through the unified launcher state.
    pub(crate) fn persist_hidden(&self) {
        self.persist_launcher_state();
    }

    pub(crate) fn persist_settings(&self) {
        if self.qa_enabled() {
            return;
        }
        if let Err(e) = self.cache.put_settings(&self.settings) {
            eprintln!("settings: failed to persist settings: {e}");
        }
    }

    /// Push the persisted Liquid Glass parameters from `self.settings` into
    /// the renderer. Called once at startup (after the renderer is created)
    /// and after any reset. Debug-only flags are never touched here.
    pub(crate) fn apply_persisted_liquid_glass_to_renderer(&mut self) {
        let Some(r) = self.renderer.as_mut() else {
            return;
        };
        let lg = &self.settings.liquid_glass;
        r.apply_persisted_liquid_glass(
            lg.enabled,
            lg.thickness,
            lg.refractive_index,
            lg.saturation,
            lg.chromatic_aberration,
            lg.blur_radius,
        );
    }

    /// Hide the launcher window and reset transient UI state (search field,
    /// scroll position, IME), but keep the process + event loop alive so it
    /// can be summoned again. Idempotent: a no-op if already hidden.
    pub(crate) fn hide(&mut self) {
        if !self.visible {
            debug_log!("hide: already hidden, no-op");
            return;
        }
        debug_log!(
            "hide: hiding window context_menu_phase={:?} active={} router={:?}",
            self.context_menu.phase,
            self.context_menu.is_active(),
            self.input_router.state(),
        );
        self.cancel_and_reset_scroll_input(super::update::ScrollLifecycleBoundary::HideWindow);
        if let Some(r) = self.renderer.as_mut() {
            r.set_backdrop_capture_active(false);
            r.window.set_visible(false);
            r.window.set_ime_allowed(false);
        }
        // Exit edit mode if active, persisting any reorder before we vanish.
        if self.editing {
            self.exit_edit_mode();
        }
        // Close the settings overlay so a re-summon starts clean.
        self.settings_open = false;
        self.settings_panel_progress = 0.0;
        self.folders = crate::features::folders::FolderFeatureState::default();
        self.folder_layout = None;
        self.folder_scroll_pending_commit = false;
        // The menu is also transient window state. Reset it synchronously on
        // hide instead of leaving an Opening/Closing menu to capture input
        // after the next summon.
        self.context_menu = crate::features::context_menu::ContextMenuState::default();
        self.clear_context_menu_presentation();
        self.pending_press = None;
        self.input_router.reset();
        // Drop any in-progress search / IME composition so the next summon
        // starts clean.
        self.control.press_close();
        self.relayout();
        // Reset scroll to page 0 so the next appearance doesn't land mid-page.
        if let Some(s) = self.scroller.as_mut() {
            s.position = 0.0;
            s.velocity = 0.0;
            s.phase = Phase::Idle;
        }
        self.last_page = 0;
        self.visible = false;
        self.request_redraw();
    }

    /// Hide the launcher after a transparent-area click and, on Windows, send
    /// a best-effort replacement click to whatever is now under the cursor.
    pub(crate) fn hide_with_click_passthrough(
        &mut self,
        button: crate::input_routing::PointerButton,
    ) {
        #[cfg(windows)]
        let click = crate::platform::windows::prepare_click_at_cursor(
            self.native_window_identity(),
            button,
        );
        self.hide();
        #[cfg(windows)]
        {
            let result = click.map_or(
                crate::input_routing::DeliveryResult::NoTarget,
                crate::platform::windows::deliver_prepared_click,
            );
            debug_log!("outside-click: delivery result={result:?}");
        }
        #[cfg(target_os = "macos")]
        {
            let result = self
                ._macos_input
                .as_ref()
                .map_or(crate::input_routing::DeliveryResult::NoTarget, |adapter| {
                    adapter.deliver_click(button)
                });
            debug_log!("outside-click: macOS delivery result={result:?}");
        }
    }

    /// Show the launcher window and steal focus. Counterpart to [`hide`].
    /// Re-centers on the primary monitor so a multi-monitor move doesn't
    /// strand the launcher on the wrong screen.
    pub(crate) fn summon(&mut self) {
        let Some(r) = self.renderer.as_mut() else {
            return;
        };
        debug_log!("summon: showing window (visible was {})", self.visible);
        r.window.set_visible(true);
        r.set_backdrop_capture_active(true);
        #[cfg(target_os = "macos")]
        {
            self.awaiting_initial_focus = true;
            self.window_focused = false;
        }
        // Steal focus. focus_window() can be silently denied by Windows when
        // the foreground already belongs to another app (common after hide()),
        // so we also allow-set-foreground + re-assert focus. If it still fails
        // the user at least sees the window appear (visible=true above) even
        // if it's not topmost.
        #[cfg(windows)]
        {
            // ASFW_ANY (-1) lifts the SetForegroundWindow restriction so any
            // process (incl. ours) can come to the front. This is what lets a
            // hotkey-triggered summon reliably raise the window instead of
            // just flashing the taskbar after the window was hidden.
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
                const ASFW_ANY: u32 = u32::MAX; // -1 as the Win32 ASFW_ANY sentinel
                let _ = AllowSetForegroundWindow(ASFW_ANY);
            }
        }
        #[cfg(target_os = "macos")]
        crate::platform::macos::integration::activate_application();
        r.window.focus_window();
        self.visible = true;
        // Record the summon time so a focus-transition artifact in the next
        // SUMMON_FOCUS_GRACE is ignored instead of instantly hiding us.
        self.last_summon = Some(Instant::now());
        self.request_redraw();
        debug_log!("summon: window shown + focus requested");
    }

    /// Execute one [`AppCommand`] at the app boundary. Preserves the historical
    /// side-effect ordering: e.g. launch hides the window first and opens the
    /// shortcut second.
    pub(super) fn execute_command(&mut self, command: AppCommand) {
        match command {
            AppCommand::RequestRedraw => self.request_redraw(),
            AppCommand::HideWindow => self.hide(),
            AppCommand::HideWithClickPassthrough(button) => {
                self.hide_with_click_passthrough(button)
            }
            AppCommand::Summon => self.summon(),
            AppCommand::LaunchApp(info) => {
                let link_path = info.link_path.clone();
                let name = info.name.clone();
                self.hide();
                match crate::platform::launch::open_shortcut(&link_path) {
                    Ok(()) => eprintln!("launched {}", name),
                    Err(err) => eprintln!(
                        "failed to launch {} ({}): {}",
                        name,
                        link_path.display(),
                        err
                    ),
                }
            }
            AppCommand::RevealApp(info) => {
                let name = info.name.clone();
                let path = reveal_target_path(&info);
                self.hide();
                match crate::platform::launch::reveal(&path) {
                    Ok(()) => eprintln!("revealed {} ({})", name, path.display()),
                    Err(err) => {
                        eprintln!("failed to reveal {} ({}): {}", name, path.display(), err)
                    }
                }
            }
            AppCommand::AskChatGpt(info) => {
                let name = info.name.clone();
                let url = chatgpt_help_url(&info);
                // Unlike launch/reveal, do NOT hide the launcher: the browser
                // opens in the background and the user may want to keep the
                // menu / grid visible while reading the answer.
                match crate::platform::launch::open_url(&url) {
                    Ok(()) => eprintln!("chatgpt help: opened for {}", name),
                    Err(err) => eprintln!("failed to open ChatGPT help for {}: {}", name, err),
                }
            }
            AppCommand::PersistSettings => self.persist_settings(),
            AppCommand::PersistUserOrder => self.persist_user_order(),
            AppCommand::PersistHidden => self.persist_hidden(),
            AppCommand::Relayout => self.relayout(),
            AppCommand::ResetIconCache => self.reset_icons(),
            // Edit-mode-consolidated side effects:
            AppCommand::SetEditing(value) => self.set_editing(value),
            AppCommand::SetDragItem(value) => self.drag_item = value,
            AppCommand::SetDragPos(x, y) => {
                self.drag_x = x;
                self.drag_y = y;
            }
            AppCommand::ResetWigglePhase => self.wiggle_phase = 0.0,
            AppCommand::CancelScroll => {
                if let Some(s) = self.scroller.as_mut() {
                    if s.phase != Phase::Idle {
                        s.phase = Phase::Idle;
                        s.velocity = 0.0;
                    }
                }
            }
            AppCommand::ClearPendingPress => self.pending_press = None,
            AppCommand::SetSortManual => {
                self.settings.sort_order = crate::domain::settings::SortOrder::Manual;
            }
            AppCommand::HideApp(app_id) => self.hide_app(&app_id),
            AppCommand::SettleToPage(page) => {
                if let Some(s) = self.scroller.as_mut() {
                    if s.phase == Phase::Idle && s.settle_to_page(page) {
                        self.request_redraw();
                    }
                }
            }
        }
    }

    /// Execute a batch of [`EditModeCommand`]s by projecting each onto the
    /// equivalent [`AppCommand`] and running it through [`execute_command`].
    ///
    /// This is the Phase 5 consolidation point: edit-mode feature logic returns
    /// `Vec<EditModeCommand>` (see [`crate::features::edit_mode`]) and the app
    /// shell runs the side effects here, so edit-mode, settings, search, and
    /// grid all share one command-execution boundary.
    ///
    /// The mapping is order-preserving: commands run in the order the feature
    /// module emitted them (e.g. `SetSortManual` before `PersistSettings`
    /// before `PersistUserOrder` on a commit), matching the historical inline
    /// `commit_reorder` sequence.
    pub(super) fn execute_edit_mode_commands(&mut self, commands: Vec<EditModeCommand>) {
        for cmd in commands {
            let app_cmd = match cmd {
                EditModeCommand::SetEditing(v) => AppCommand::SetEditing(v),
                EditModeCommand::SetDragItem(v) => AppCommand::SetDragItem(v),
                EditModeCommand::SetDragPos(x, y) => AppCommand::SetDragPos(x, y),
                EditModeCommand::ResetWigglePhase => AppCommand::ResetWigglePhase,
                EditModeCommand::CancelScroll => AppCommand::CancelScroll,
                EditModeCommand::ClearPendingPress => AppCommand::ClearPendingPress,
                EditModeCommand::Relayout => AppCommand::Relayout,
                EditModeCommand::RequestRedraw => AppCommand::RequestRedraw,
                EditModeCommand::PersistUserOrder => AppCommand::PersistUserOrder,
                EditModeCommand::PersistHidden => AppCommand::PersistHidden,
                EditModeCommand::PersistSettings => AppCommand::PersistSettings,
                EditModeCommand::SetSortManual => AppCommand::SetSortManual,
                EditModeCommand::HideApp(id) => AppCommand::HideApp(id),
                EditModeCommand::SettleToPage(page) => AppCommand::SettleToPage(page),
            };
            self.execute_command(app_cmd);
        }
    }

    /// `editing = value` (idempotent). Logs the first transition only. Exposed
    /// as a narrow helper so [`execute_command`] can apply `SetEditing` without
    /// reaching into the field directly; the log-on-first-transition behavior
    /// is preserved.
    fn set_editing(&mut self, value: bool) {
        let was_editing = self.editing;
        self.editing = value;
        if value && !was_editing {
            debug_log!("edit-mode: entered");
        } else if !value && was_editing {
            debug_log!("edit-mode: exited");
        }
    }
}

/// Pick the path the OS file manager should select for a reveal.
///
/// Windows selects the resolved target (the real `.exe` the `.lnk` expands
/// to) when the shortcut resolved — the user sees the app's install folder
/// with the app selected, which is the better experience — and falls back to
/// the `.lnk` itself when the target could not be resolved. macOS selects the
/// `.app` bundle (`link_path`): `open -R` on the executable inside a bundle
/// would point Finder at `Contents/MacOS` instead of the app itself.
fn reveal_target_path(info: &AppLaunchInfo) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        if info.resolved_target.as_os_str().is_empty() {
            info.link_path.clone()
        } else {
            info.resolved_target.clone()
        }
    }
    #[cfg(target_os = "macos")]
    {
        info.link_path.clone()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        info.link_path.clone()
    }
}

/// Build the structured prompt sent to ChatGPT for the "how to use this app"
/// context-menu action. Kept as a pure function (no I/O, no encoding) so it is
/// straightforward to unit-test the template and field interpolation. Unknown
/// metadata is omitted instead of being replaced with a guessed placeholder.
///
/// The prompt is intentionally sectioned (overview → features → tips →
/// pitfalls → integrations) and pinned to the app's name + version + platform
/// so the answer is specific rather than generic.
pub(crate) fn build_chatgpt_prompt(info: &AppLaunchInfo) -> String {
    let platform = platform_label();
    // OS locale as a BCP-47 tag (e.g. "ja-JP", "en-US"). Falls back to "en-US"
    // when the platform does not report one.
    let locale = sys_locale::get_locale()
        .filter(|locale| !locale.trim().is_empty())
        .unwrap_or_else(|| "en-US".to_owned());
    // Human-readable language name for "write the response in <language>".
    let language_name = locale_language_name(&locale);

    // Metadata comes from installed applications, but it is still external
    // input. Keep a malformed Info.plist/version resource from injecting a
    // newline or quote into the prompt structure.
    let name = prompt_field(&info.name);
    let name = if name.is_empty() {
        "this application".to_owned()
    } else {
        name
    };
    let version = prompt_field(&info.version);
    let publisher = prompt_field(&info.publisher);
    let identifier = prompt_field(&info.identifier);
    let research_target = if version.is_empty() {
        format!("{name} on {platform}")
    } else {
        format!("{name} {version} on {platform}")
    };
    // Application-information block: only lines whose value was read. The
    // developer (publisher) and bundle id / product name disambiguate same-named
    // apps (e.g. Cinema 4D's "Commandline" → MAXON / net.maxon.commandline).
    let mut app_info = format!("App name: {name}\nPlatform: {platform}\n");
    if !version.is_empty() {
        app_info.push_str(&format!("Version: {version}\n"));
    }
    if !publisher.is_empty() {
        app_info.push_str(&format!("Developer: {publisher}\n"));
    }
    if !identifier.is_empty() {
        app_info.push_str(&format!("{}: {identifier}\n", identifier_label()));
    }

    format!(
        "How to use \"{name}\"\
        \n\n\
## ROLE\
        \n\nYou are an expert on {platform} desktop applications. Create a practical, visually easy-to-understand guide for a user who wants to master {name}.\
        \n\n## TASK\
        \n\nResearch and explain {research_target}.\
        \n\nDo not provide only a generic feature list. Explain how the application is actually used, what each major feature is useful for, and how the features fit into real workflows.\
        \n\nCover the following sections:\
        \n\n1. Overview\
        \n\n   * What {name} is\
        \n   * What it is used for\
        \n   * Who it is suitable for\
        \n   * What makes it different from similar or competing applications\
        \n\n2. What you can do\
        \n\n   * Main features\
        \n   * What each feature is useful for\
        \n   * Concrete step-by-step workflow examples\
        \n   * Typical use cases for the people who rely on this kind of application\
        \n\n3. Tips and best practices\
        \n\n   * Useful keyboard shortcuts\
        \n   * Mouse and trackpad gestures\
        \n   * Hidden or less obvious features\
        \n   * Recommended settings\
        \n   * Efficient workflows for the most common tasks\
        \n\n4. Common pitfalls\
        \n\n   * Frequent mistakes\
        \n   * Features users commonly misunderstand\
        \n   * File format or compatibility limitations\
        \n   * Problems related to the application's core operations (opening, saving, exporting, configuration, performance) and how to avoid or resolve each one\
        \n\n5. Integration with {platform} and other apps\
        \n\n{integration_bullets}\
        \n\n## IMAGES\
        \n\nUse web search and image search to find relevant images of {name}.\
        \n\nDisplay the images directly in the response. Do not provide only links to pages containing the images.\
        \n\nInclude the following when reliable images are available:\
        \n\n* The official {name} app icon or promotional image near the beginning\
        \n* Two to four screenshots showing the main interface and important features\
        \n* Screenshots that help explain the application's signature features and workflows\
        \n* A short {language_name} caption under each image\
        \n* The source of each image\
        \n\nPlace each image close to the section it helps explain.\
        \n\nPrioritize image sources in this order:\
        \n\n1. The official {name} or developer website\
        \n2. {store_name}\
        \n3. Official documentation or support pages\
        \n4. Reputable software-review websites\
        \n\nDo not generate fictional screenshots.\
        \n\nDo not use images from unrelated applications or unverified versions.\
        \n\nWhen the exact application version shown in an image cannot be confirmed, clearly state that the screenshot may show a nearby version.\
        \n\nWhen images cannot be displayed in the current environment, clearly state that limitation and provide labeled source links instead. Do not silently omit the image section.\
        \n\n## RESEARCH REQUIREMENTS\
        \n\nSearch the web before answering.\
        \n\nPrioritize current and reliable sources, especially:\
        \n\n* The official developer website\
        \n* Official documentation\
        \n* Official support pages\
        \n* {store_name}\
        \n* Release notes\
        \n\nVerify that important information applies to {research_target}.\
        \n\nDistinguish between:\
        \n\n* Officially confirmed features\
        \n* Features confirmed only for a nearby version\
        \n* Reasonable workflow recommendations\
        \n* Unverified information\
        \n\nDo not invent keyboard shortcuts, gestures, menu names, features, export options, or compatibility details.\
        \n\nCite sources for version-specific features, limitations, shortcuts, and important claims.\
        \n\nWhen reliable information cannot be found, say so clearly instead of guessing.\
        \n\n## RESPONSE STYLE\
        \n\nWrite the response in natural {language_name}.\
        \n\nMake it easy to scan and understand visually.\
        \n\nUse:\
        \n\n* Clear section headings\
        \n* Short paragraphs\
        \n* Concise bullet points\
        \n* Numbered steps for procedures\
        \n* Bold text for important controls, menu names, and warnings\
        \n* Tables only when they make comparison easier\
        \n\nBegin with:\
        \n\n1. The official app image or promotional image\
        \n2. A two- or three-sentence summary explaining what {name} is\
        \n3. A brief \"{name} is especially useful for\" section\
        \n\nFor each major feature, explain:\
        \n\n* What it does\
        \n* When to use it\
        \n* How to use it\
        \n* A concrete example\
        \n\nAvoid generic {platform} advice that is not specifically useful for {name}.\
        \n\nDo not make the answer unnecessarily brief. Prioritize clarity, practical usefulness, and visual readability.\
        \n\n## OUTPUT LANGUAGE\
        \n\n{language_name}, {locale}\
        \n\n## APPLICATION INFORMATION\
        \n\n{app_info}",
        name = name,
        research_target = research_target,
        platform = platform,
        language_name = language_name,
        locale = locale,
        store_name = app_store_name(),
        integration_bullets = integration_bullets(),
        app_info = app_info.trim_end(),
    )
}

/// Map the compile-time target into a human-friendly platform label.
fn platform_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "macOS"
    } else if cfg!(windows) {
        "Windows"
    } else {
        "this platform"
    }
}

/// The platform's first-party app store name (used in source priorities).
fn app_store_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "The Mac App Store"
    } else if cfg!(windows) {
        "The Microsoft Store"
    } else {
        "The platform's app store"
    }
}

/// Label for the vendor identifier field, matching what each platform actually
/// exposes (macOS bundle id vs Windows product name).
fn identifier_label() -> &'static str {
    if cfg!(target_os = "macos") {
        "Bundle identifier"
    } else {
        "Product name"
    }
}

/// The "Integration with <platform> and other apps" bullets, localized per OS
/// so the suggestions name real first-party tools the user actually has.
fn integration_bullets() -> &'static str {
    if cfg!(target_os = "macos") {
        "   * Finder\n\
         \x20   * Quick Look\n\
         \x20   * Music or Apple Music\n\
         \x20   * GarageBand, Logic Pro, or other DAWs where relevant\n\
         \x20   * Other audio, video, image, or document apps in this app's category\n\
         \x20   * macOS keyboard shortcuts, Spaces, Split View, Stage Manager, and other useful operating-system features"
    } else if cfg!(windows) {
        "   * File Explorer\n\
         \x20   * Preview pane and Windows Search\n\
         \x20   * Windows Media Player or media apps where relevant\n\
         \x20   * DAWs, editors, or other apps in this app's category\n\
         \x20   * Windows keyboard shortcuts, Task View, Snap Layouts, and other useful operating-system features"
    } else {
        "   * The platform's file manager and built-in search\n\
         \x20   * Media apps and other apps in this app's category\n\
         \x20   * Keyboard shortcuts and window-management features"
    }
}

/// Map a BCP-47 locale tag to a human-readable language name used in the
/// "write the response in <language>" and "output language" lines. Falls back
/// to the locale tag itself when unknown.
fn locale_language_name(locale: &str) -> String {
    let lower = locale.to_ascii_lowercase();
    let primary = lower.split(['-', '_']).next().unwrap_or("en");
    match primary {
        "ja" => "Japanese".to_owned(),
        "en" => "English".to_owned(),
        "zh" => "Chinese".to_owned(),
        "ko" => "Korean".to_owned(),
        "es" => "Spanish".to_owned(),
        "fr" => "French".to_owned(),
        "de" => "German".to_owned(),
        "it" => "Italian".to_owned(),
        "pt" => "Portuguese".to_owned(),
        "ru" => "Russian".to_owned(),
        "ar" => "Arabic".to_owned(),
        "nl" => "Dutch".to_owned(),
        "pl" => "Polish".to_owned(),
        "tr" => "Turkish".to_owned(),
        "vi" => "Vietnamese".to_owned(),
        "th" => "Thai".to_owned(),
        "id" => "Indonesian".to_owned(),
        "hi" => "Hindi".to_owned(),
        "" => "English".to_owned(),
        other => other.to_owned(),
    }
}

/// Keep application metadata as a single safe, readable prompt field.
fn prompt_field(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('"', "'")
}

/// Build the full `https://chatgpt.com/?q=<encoded>` URL for the help action.
pub(crate) fn chatgpt_help_url(info: &AppLaunchInfo) -> String {
    let prompt = build_chatgpt_prompt(info);
    format!("https://chatgpt.com/?q={}", urlencoding::encode(&prompt))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::app_id::AppId;
    use crate::domain::launcher_item::LauncherItem;
    use crate::features::edit_mode::EditModeCommand;

    fn launch_info() -> AppLaunchInfo {
        AppLaunchInfo {
            name: "X".to_string(),
            link_path: std::path::PathBuf::from("x.lnk"),
            resolved_target: std::path::PathBuf::from("x.exe"),
            version: "1.0".to_string(),
            publisher: String::new(),
            identifier: String::new(),
        }
    }

    /// The reveal path policy: Windows selects the resolved target (the real
    /// `.exe`) when the shortcut resolved, falling back to the `.lnk`; macOS
    /// always selects the `.app` bundle (`link_path`), never the executable
    /// buried in `Contents/MacOS`.
    #[test]
    fn reveal_target_path_follows_platform_policy() {
        #[cfg(windows)]
        {
            assert_eq!(
                reveal_target_path(&launch_info()),
                std::path::PathBuf::from("x.exe")
            );
            // Unresolvable shortcut → the .lnk itself.
            let unresolved = AppLaunchInfo {
                resolved_target: std::path::PathBuf::new(),
                ..launch_info()
            };
            assert_eq!(
                reveal_target_path(&unresolved),
                std::path::PathBuf::from("x.lnk")
            );
        }
        #[cfg(not(windows))]
        {
            // macOS (and other platforms): the bundle path, even when a
            // resolved target exists.
            assert_eq!(
                reveal_target_path(&launch_info()),
                std::path::PathBuf::from("x.lnk")
            );
        }
    }

    /// The pure projection from `EditModeCommand` to `AppCommand` is total:
    /// every edit-mode variant maps to exactly one app command. This test
    /// pins the mapping so a future edit-mode variant cannot silently drop out
    /// of the command boundary (the compiler exhaustive-match already enforces
    /// this at the `match`, but the test documents the intended mapping).
    #[test]
    fn edit_mode_command_maps_exhaustively_and_order_preserving() {
        let id = AppId::from_normalized("app-a");
        let edit_cmds = vec![
            EditModeCommand::SetEditing(true),
            EditModeCommand::SetDragItem(Some(LauncherItem::App(id.clone()))),
            EditModeCommand::SetDragPos(10.0, 20.0),
            EditModeCommand::ResetWigglePhase,
            EditModeCommand::CancelScroll,
            EditModeCommand::ClearPendingPress,
            EditModeCommand::Relayout,
            EditModeCommand::RequestRedraw,
            EditModeCommand::PersistUserOrder,
            EditModeCommand::PersistHidden,
            EditModeCommand::PersistSettings,
            EditModeCommand::SetSortManual,
            EditModeCommand::HideApp(id.clone()),
            EditModeCommand::SettleToPage(2),
        ];
        let mapped: Vec<AppCommand> = edit_cmds
            .iter()
            .map(|c| match c {
                EditModeCommand::SetEditing(v) => AppCommand::SetEditing(*v),
                EditModeCommand::SetDragItem(v) => AppCommand::SetDragItem(v.clone()),
                EditModeCommand::SetDragPos(x, y) => AppCommand::SetDragPos(*x, *y),
                EditModeCommand::ResetWigglePhase => AppCommand::ResetWigglePhase,
                EditModeCommand::CancelScroll => AppCommand::CancelScroll,
                EditModeCommand::ClearPendingPress => AppCommand::ClearPendingPress,
                EditModeCommand::Relayout => AppCommand::Relayout,
                EditModeCommand::RequestRedraw => AppCommand::RequestRedraw,
                EditModeCommand::PersistUserOrder => AppCommand::PersistUserOrder,
                EditModeCommand::PersistHidden => AppCommand::PersistHidden,
                EditModeCommand::PersistSettings => AppCommand::PersistSettings,
                EditModeCommand::SetSortManual => AppCommand::SetSortManual,
                EditModeCommand::HideApp(i) => AppCommand::HideApp(i.clone()),
                EditModeCommand::SettleToPage(p) => AppCommand::SettleToPage(*p),
            })
            .collect();
        // The mapping is 1:1 and order-preserving.
        assert_eq!(mapped.len(), edit_cmds.len());
        assert!(matches!(mapped[0], AppCommand::SetEditing(true)));
        assert!(matches!(mapped[11], AppCommand::SetSortManual));
        assert!(matches!(mapped[12], AppCommand::HideApp(_)));
    }

    /// `commit_drag` emits `SetSortManual` *before* the persist commands so the
    /// persisted settings carry the new sort order. This is the historical
    /// `commit_reorder` sequence (`sort_order = Manual` → `persist_settings` →
    /// `persist_user_order`) and the Phase 5 consolidation must preserve it.
    #[test]
    fn commit_drag_command_order_is_sort_manual_before_persist() {
        use crate::features::edit_mode::{commit_drag, EditModeState};
        let mut state = EditModeState {
            editing: true,
            drag_item: Some(LauncherItem::App(AppId::from_normalized("dragged"))),
            ..EditModeState::default()
        };
        let cmds = commit_drag(&state);
        assert_eq!(
            cmds,
            vec![
                EditModeCommand::SetSortManual,
                EditModeCommand::PersistSettings,
                EditModeCommand::PersistUserOrder,
            ]
        );
        // No drag → no persist (the historical commit_reorder was only called
        // when a drag was in flight).
        state.drag_item = None;
        assert!(commit_drag(&state).is_empty());
    }

    /// `enter` emits the entry side effects in the historical order:
    /// SetEditing → ClearPendingPress → ResetWigglePhase → CancelScroll, then
    /// the optional app lift (SetDragApp + SetDragPos), then Relayout +
    /// RequestRedraw.
    #[test]
    fn enter_command_order_matches_historical_entry_sequence() {
        use crate::features::edit_mode::{enter, EditModeState, PointerSnapshot};
        let mut state = EditModeState::default();
        let visible = vec![AppId::from_normalized("a"), AppId::from_normalized("b")];
        let cmds = enter(
            &mut state,
            Some(1),
            &visible,
            PointerSnapshot::new(50.0, 60.0),
        );
        // The entry core always comes first, in this order.
        assert_eq!(cmds[0], EditModeCommand::SetEditing(true));
        assert_eq!(cmds[1], EditModeCommand::ClearPendingPress);
        assert_eq!(cmds[2], EditModeCommand::ResetWigglePhase);
        assert_eq!(cmds[3], EditModeCommand::CancelScroll);
        // Then the app lift.
        assert_eq!(
            cmds[4],
            EditModeCommand::SetDragItem(Some(LauncherItem::App(AppId::from_normalized("b"))))
        );
        assert_eq!(cmds[5], EditModeCommand::SetDragPos(50.0, 60.0));
        // Then relayout + redraw.
        assert_eq!(cmds[6], EditModeCommand::Relayout);
        assert_eq!(cmds[7], EditModeCommand::RequestRedraw);
        assert_eq!(cmds.len(), 8);
    }

    /// `exit` with an in-flight drag runs the commit commands *before* the
    /// exit transitions, so the drop is persisted before editing is cleared.
    #[test]
    fn exit_with_drag_runs_commit_before_clearing() {
        use crate::features::edit_mode::{commit_drag, exit, EditModeState};
        let mut state = EditModeState {
            editing: true,
            drag_item: Some(LauncherItem::App(AppId::from_normalized("dragged"))),
            ..EditModeState::default()
        };
        let commit = commit_drag(&state);
        let cmds = exit(&mut state, commit);
        // Commit (SetSortManual, PersistSettings, PersistUserOrder) first…
        assert_eq!(cmds[0], EditModeCommand::SetSortManual);
        assert_eq!(cmds[1], EditModeCommand::PersistSettings);
        assert_eq!(cmds[2], EditModeCommand::PersistUserOrder);
        // …then the exit transitions.
        assert_eq!(cmds[3], EditModeCommand::SetEditing(false));
        assert_eq!(cmds[4], EditModeCommand::SetDragItem(None));
        assert_eq!(cmds[5], EditModeCommand::ClearPendingPress);
        assert_eq!(cmds[6], EditModeCommand::Relayout);
        assert_eq!(cmds[7], EditModeCommand::RequestRedraw);
    }

    /// The prompt opens with a plain-English "How to use" header (so the
    /// browser tab / ChatGPT conversation title is meaningful) and keeps the
    /// structured section template intact.
    #[test]
    fn chatgpt_prompt_embeds_name_version_and_sections() {
        let info = AppLaunchInfo {
            name: "DaisyDisk".to_string(),
            link_path: std::path::PathBuf::from("/Applications/DaisyDisk.app"),
            resolved_target: std::path::PathBuf::new(),
            version: "4.21".to_string(),
            publisher: String::new(),
            identifier: String::new(),
        };
        let prompt = build_chatgpt_prompt(&info);
        // Opens with the human-readable title.
        assert!(
            prompt.starts_with("How to use \"DaisyDisk\""),
            "prompt should start with the title:\n{prompt}"
        );
        // Every section heading is present.
        for heading in [
            "## ROLE",
            "## TASK",
            "## IMAGES",
            "## RESEARCH REQUIREMENTS",
            "## RESPONSE STYLE",
            "## OUTPUT LANGUAGE",
            "## APPLICATION INFORMATION",
            "1. Overview",
            "2. What you can do",
            "3. Tips and best practices",
            "4. Common pitfalls",
            "5. Integration with",
        ] {
            assert!(
                prompt.contains(heading),
                "prompt missing section heading {heading:?}:\n{prompt}"
            );
        }
        // App name and version are interpolated into the research target and
        // the application-information block.
        assert!(prompt.contains("\"DaisyDisk\""));
        assert!(prompt.contains("DaisyDisk 4.21 on"));
        assert!(prompt.contains("App name: DaisyDisk"));
        assert!(prompt.contains("Version: 4.21"));
        // The prompt is fully English (no Japanese-only headings left).
        assert!(!prompt.contains("概要"));
        assert!(!prompt.contains("チップス"));
    }

    /// An unknown version omits the version from the research target and the
    /// application-information block, so ChatGPT is not fed a placeholder.
    #[test]
    fn chatgpt_prompt_omits_version_when_unknown() {
        let info = AppLaunchInfo {
            name: "MysteryApp".to_string(),
            link_path: std::path::PathBuf::new(),
            resolved_target: std::path::PathBuf::new(),
            version: String::new(),
            publisher: String::new(),
            identifier: String::new(),
        };
        let prompt = build_chatgpt_prompt(&info);
        assert!(!prompt.contains("Version:"));
        assert!(!prompt.contains("unknown"));
        // The research target falls back to "<name> on <platform>".
        assert!(prompt.contains("MysteryApp on"));
    }

    /// Developer and bundle id lines appear only when populated, so a same-named
    /// app (Cinema 4D's "Commandline") is disambiguated for ChatGPT.
    #[test]
    fn chatgpt_prompt_includes_publisher_and_identifier_when_present() {
        let info = AppLaunchInfo {
            name: "Commandline".to_string(),
            link_path: std::path::PathBuf::new(),
            resolved_target: std::path::PathBuf::new(),
            version: "2025.3".to_string(),
            publisher: "© 1989-2025 MAXON Computer GmbH".to_string(),
            identifier: "net.maxon.commandline".to_string(),
        };
        let prompt = build_chatgpt_prompt(&info);
        assert!(prompt.contains("Developer: © 1989-2025 MAXON Computer GmbH"));
        assert!(prompt.contains("net.maxon.commandline"));
        assert!(prompt.contains("\"Commandline\""));
    }

    #[test]
    fn chatgpt_prompt_sanitizes_external_metadata_fields() {
        let info = AppLaunchInfo {
            name: "  Audio\nPlayer\"  ".to_string(),
            link_path: std::path::PathBuf::new(),
            resolved_target: std::path::PathBuf::new(),
            version: "  1.0\t beta  ".to_string(),
            publisher: "Acme\nAudio".to_string(),
            identifier: "com.acme.\"player\"".to_string(),
        };
        let prompt = build_chatgpt_prompt(&info);
        assert!(prompt.contains("How to use \"Audio Player'\""));
        assert!(prompt.contains("Audio Player' 1.0 beta on"));
        assert!(prompt.contains("Developer: Acme Audio"));
        assert!(prompt.contains("com.acme.'player'"));
        assert!(!prompt.contains("Audio\nPlayer"));
    }

    /// The full URL stays within a safe length budget even with the larger
    /// research/image/research-requirements template, so browsers accept it.
    #[test]
    fn chatgpt_help_url_stays_within_browser_limits() {
        let info = AppLaunchInfo {
            name: "Commandline".to_string(),
            link_path: std::path::PathBuf::new(),
            resolved_target: std::path::PathBuf::new(),
            version: "2025.3".to_string(),
            publisher: "© 1989-2025 MAXON Computer GmbH".to_string(),
            identifier: "net.maxon.commandline".to_string(),
        };
        let url = chatgpt_help_url(&info);
        // Common browsers accept URLs up to ~8 KB (Chrome's address-bar limit is
        // ~2 MB; server-side query-string limits vary but 8 KB is safe). The
        // percent-encoded prompt roughly triples in size, so cap at 8 KB.
        assert!(
            url.len() <= 8_192,
            "URL is {} bytes, over the 8 KB budget",
            url.len()
        );
    }

    /// The help URL percent-encodes the prompt into the `q` query parameter,
    /// so spaces and the section markers survive the trip to the browser.
    #[test]
    fn chatgpt_help_url_percent_encodes_the_prompt() {
        let info = AppLaunchInfo {
            name: "DaisyDisk".to_string(),
            link_path: std::path::PathBuf::new(),
            resolved_target: std::path::PathBuf::new(),
            version: "4.21".to_string(),
            publisher: String::new(),
            identifier: String::new(),
        };
        let url = chatgpt_help_url(&info);
        assert!(url.starts_with("https://chatgpt.com/?q="));
        // Spaces are encoded as %20, not left raw.
        assert!(!url.contains(' '), "URL must not contain raw spaces: {url}");
        // The decoded query round-trips back to the structured prompt.
        let encoded = &url["https://chatgpt.com/?q=".len()..];
        let decoded = urlencoding::decode(encoded).expect("valid percent-encoding");
        assert!(decoded.contains("## TASK"));
        assert!(decoded.contains("\"DaisyDisk\""));
    }
}
