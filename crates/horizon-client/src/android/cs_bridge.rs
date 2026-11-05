//! CS Launcher Plus host mode (cargo feature `cs-host`).
//!
//! CS Launcher Plus embeds `libhorizon_android.so` and drives it from its own UI:
//! the Rust launcher pages, account panel and catalog are never shown. Two entry
//! points exist:
//!
//! * **Play.** CS starts a `NativeActivity` with the string extra `cs_launch`
//!   (JSON, see [`Launch`]). `android_main` hands that to [`run`], which prepares
//!   the engine exactly as the Rust launcher's Play button does and runs the game
//!   as the only `ApplicationHandler`. Leaving the game ends the process.
//! * **Install.** CS calls the JNI function
//!   `HorizonNative.nativeInstall` from a foreground service (no window); it wraps
//!   `official_install::install_with_progress` and reports every stage back to Java.
//!
//! Everything else (installer, renderer, input, diagnostics) is the upstream code.
#![cfg(target_os = "android")]

use jni::{
    JNIEnv, JavaVM,
    objects::{JClass, JObject, JString, JValue},
    sys::{jboolean, jstring},
};
use serde::Deserialize;
use std::{
    path::{Path, PathBuf},
    sync::mpsc,
};
use winit::{
    application::ApplicationHandler,
    event::{DeviceEvent, DeviceId, StartCause, WindowEvent},
    event_loop::ActiveEventLoop,
    platform::android::activity::AndroidApp,
    window::WindowId,
};

/// Intent extra carrying the launch request.
pub const INTENT_EXTRA: &str = "cs_launch";
/// Written next to `android-error.txt` when the game ends; CS reads it after the activity finishes.
const EXIT_FILE: &str = "cs-exit.json";

/// The launch request CS Launcher Plus serialises into the `cs_launch` intent extra.
/// Paths are absolute. Missing optional fields take the Rust launcher's defaults.
#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct Launch {
    /// Engine home: `engine-data.zip`, `.local/horizon-client/<version>`, diagnostics.
    pub home: String,
    /// Download / shader cache directory (`Context.getCacheDir()` or a sub-folder).
    pub cache: String,
    /// Game folder of the CS profile: saves, options.txt, resourcepacks, shaderpacks, mods.
    pub game_home: String,
    /// Prepared resources; defaults to `<home>/.local/horizon-client/<version>`.
    pub resources: String,
    /// Path of the `engine-data.zip` shipped in the CS APK assets (copied into `home` when newer).
    pub engine_data: String,
    /// Session file written by CS (Launch API format, mode 0600). Empty for offline play.
    pub session_file: String,
    /// Offline player name when no session file is given.
    pub offline_name: String,
    /// `host[:port]` to join at start.
    pub server: String,
    /// `vulkan`, `vulkan-compatibility` (default) or `opengl`. Empty keeps `android-launcher.json`.
    pub renderer: String,
    /// Extra `horizon-client` options (`--render-distance 8`, `--quickPlaySingleplayer World`).
    pub extra_arguments: Vec<String>,
    /// CS Launcher's own touch controls drive the game (`HorizonNative.nativeHost*`); the
    /// engine draws no touch overlay and ignores window touches.
    pub host_input: bool,
    /// Resolution scale in percent (25–100); 0 keeps the value of `android-launcher.json`.
    pub render_scale: u32,
    /// Local skin PNG (64x64 or 64x32) for the player where no account/server texture applies.
    pub skin: String,
    /// `slim` (Alex) or `classic` (Steve) for `skin`.
    pub skin_model: String,
    /// Local cape PNG (64x32 or 22x17) where the profile has none.
    pub cape: String,
}

impl Launch {
    fn home(&self) -> PathBuf {
        PathBuf::from(&self.home)
    }
    fn resources(&self) -> PathBuf {
        if self.resources.is_empty() {
            self.home()
                .join(".local/horizon-client")
                .join(horizon_core::VANILLA_VERSION)
        } else {
            PathBuf::from(&self.resources)
        }
    }
    fn renderer(&self, saved: crate::catalog::RendererMode) -> crate::catalog::RendererMode {
        match self.renderer.as_str() {
            "vulkan" => crate::catalog::RendererMode::Vulkan,
            "opengl" => crate::catalog::RendererMode::OpenGl,
            "vulkan-compatibility" => crate::catalog::RendererMode::VulkanCompatibility,
            _ => saved,
        }
    }
}

/// Reads the `cs_launch` intent extra of the hosting activity, if CS started it.
pub fn launch_from_intent(app: &AndroidApp) -> Option<Launch> {
    let json = intent_extra(app, INTENT_EXTRA)?;
    match serde_json::from_str::<Launch>(&json) {
        Ok(launch) if !launch.home.is_empty() && !launch.game_home.is_empty() => Some(launch),
        Ok(_) => {
            log::error!("cs_launch: home and game_home are required");
            None
        }
        Err(error) => {
            log::error!("cs_launch: invalid JSON: {error}");
            None
        }
    }
}

fn intent_extra(app: &AndroidApp, key: &str) -> Option<String> {
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) }.ok()?;
    let mut env = vm.attach_current_thread().ok()?;
    let result = env.with_local_frame(16, |env| -> jni::errors::Result<Option<String>> {
        let activity = unsafe { JObject::from_raw(app.activity_as_ptr().cast()) };
        let intent = env
            .call_method(&activity, "getIntent", "()Landroid/content/Intent;", &[])?
            .l()?;
        if intent.is_null() {
            return Ok(None);
        }
        let name = env.new_string(key)?;
        let value = env
            .call_method(
                &intent,
                "getStringExtra",
                "(Ljava/lang/String;)Ljava/lang/String;",
                &[JValue::Object(&name)],
            )?
            .l()?;
        if value.is_null() {
            return Ok(None);
        }
        let text: String = env.get_string(&JString::from(value))?.into();
        Ok(Some(text))
    });
    match result {
        Ok(value) => value,
        Err(error) => {
            let _ = env.exception_clear();
            log::warn!("cs_launch intent: {error}");
            None
        }
    }
}

/// Copies the APK's engine tables into `home/engine-data.zip` when missing or different in size.
fn stage_engine_data(source: &str, home: &Path) -> std::io::Result<()> {
    if source.is_empty() {
        return Ok(());
    }
    let source = Path::new(source);
    let target = home.join("engine-data.zip");
    let same = match (std::fs::metadata(source), std::fs::metadata(&target)) {
        (Ok(a), Ok(b)) => a.len() == b.len(),
        _ => false,
    };
    if same {
        return Ok(());
    }
    std::fs::create_dir_all(home)?;
    let partial = home.join("engine-data.partial");
    std::fs::copy(source, &partial)?;
    std::fs::rename(partial, target)
}

/// A short, stable folder name for a profile's overlay cache (FNV-1a of its path).
fn overlay_key(game_home: &std::path::Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in game_home.to_string_lossy().bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let name = game_home
        .file_name()
        .map(|n| n.to_string_lossy().chars().filter(|c| c.is_ascii_alphanumeric()).take(24).collect::<String>())
        .unwrap_or_default();
    format!("{name}-{hash:016x}")
}

fn write_exit(home: &Path, error: Option<&str>) {
    let (pushed, processed) = horizon_client::android_runtime::host_stats();
    let geometry = horizon_client::android_runtime::host_geometry().map(|g| {
        serde_json::json!({
            "window": g.window, "render": g.render, "origin": g.origin, "scale": g.scale,
        })
    });
    let body = serde_json::json!({
        "ok": error.is_none(),
        "error": error,
        "engine": crate::backend::VERSION,
        "cs_engine": cs_brand::full_version(crate::backend::VERSION),
        "minecraft": horizon_core::VANILLA_VERSION,
        "host_input": {"pushed": pushed, "processed": processed, "geometry": geometry},
    });
    let _ = std::fs::write(home.join(EXIT_FILE), body.to_string());
}

/// Runs the game for a CS launch request. Never returns: the process exits like `android_main`.
pub fn run(app: AndroidApp, launch: Launch) {
    let home = launch.home();
    let game_home = PathBuf::from(&launch.game_home);
    crate::backend::initialize(home.clone());
    if let Err(error) = std::fs::create_dir_all(&home).and_then(|()| std::fs::create_dir_all(&game_home)) {
        log::error!("Cannot create {}: {error}", home.display());
        write_exit(&home, Some(&error.to_string()));
        std::process::exit(2);
    }
    if let Err(error) = crate::diagnostics::initialize(
        &home,
        serde_json::json!({"host": "cs-launcher-plus", "profile": launch.game_home}),
    ) {
        log::error!("Cannot initialize local diagnostics: {error}");
    }
    if let Ok(vm) = unsafe { JavaVM::from_raw(app.vm_as_ptr().cast()) } {
        crate::secure_store::initialize(vm);
    }
    crate::APP.with(|slot| *slot.borrow_mut() = Some(app));
    let cache = if launch.cache.is_empty() {
        crate::platform::cache_dir()
    } else {
        Some(PathBuf::from(&launch.cache))
    };
    let Some(cache) = cache else {
        log::error!("cs_launch: no cache directory");
        write_exit(&home, Some("no cache directory"));
        std::process::exit(2);
    };
    let _ = std::fs::create_dir_all(&cache);
    horizon_client::android_runtime::configure_cache_dir(cache.clone());
    // Overlay roots (resource packs, content mods) need symlinks: internal storage, one
    // folder per profile.
    let overlay = home.join("overlay").join(overlay_key(&game_home));
    let _ = std::fs::create_dir_all(&overlay);
    horizon_client::android_runtime::configure_overlay_dir(overlay);
    horizon_client::official_install::configure_download_cache_dir(cache);
    horizon_client::android_runtime::install_clipboard(
        horizon_client::android_runtime::ClipboardHooks {
            get: crate::platform::clipboard_get,
            set: crate::platform::clipboard_set,
        },
    );
    if let Err(error) = std::env::set_current_dir(&home) {
        log::error!("Cannot open {}: {error}", home.display());
        write_exit(&home, Some(&error.to_string()));
        std::process::exit(2);
    }
    horizon_client::crash::install(home.clone(), "client");
    horizon_client::cs_friends::configure_dir(home.clone());
    crate::exit_reports::install(&home);
    let _ = std::fs::remove_file(home.join("android-error.txt"));
    let _ = std::fs::remove_file(home.join(EXIT_FILE));
    if let Err(error) = stage_engine_data(&launch.engine_data, &home) {
        log::error!("Cannot stage engine data: {error}");
    }

    let result = (|| -> horizon_client::Result<()> {
        if !horizon_client::official_install::ready(&home) {
            return Err("Horizon engine is not installed. Install it from Masterix first.".into());
        }
        let mut config = crate::catalog::Config::load(&home);
        // The same startup recovery as the Rust launcher: an interrupted Vulkan start falls back to GLES.
        horizon_client::android_runtime::graphics_startup::configure(home.clone());
        if horizon_client::android_runtime::graphics_startup::recover(&home) {
            config.renderer = crate::catalog::RendererMode::OpenGl;
            let _ = config.save(&home);
            log::warn!("Previous Vulkan startup was interrupted; using OpenGL ES");
        }
        let renderer = launch.renderer(config.renderer);
        // CS Skin & Cape: the launcher's selection for this profile (hot-switched later over
        // `nativeHostSkin`). Offline/local accounts see it always; Microsoft accounts keep their
        // official profile textures and only fill a missing cape with it.
        let path = |p: &str| (!p.is_empty()).then(|| PathBuf::from(p));
        if let Err(e) =
            horizon_client::skins::set_offline_skin(path(&launch.skin).as_deref(), &launch.skin_model)
        {
            log::warn!("Masterix skin not loaded: {e}");
        }
        if let Err(e) = horizon_client::skins::set_offline_cape(path(&launch.cape).as_deref()) {
            log::warn!("Masterix cape not loaded: {e}");
        }
        if launch.host_input {
            // Mouse + keyboard semantics: cursor in menus, captured look in the world.
            crate::configure_input_mode(crate::catalog::InputHost::KeyboardMouse);
            horizon_client::android_runtime::configure_host(true);
        } else {
            crate::configure_input_mode(config.input_host);
        }
        crate::device_runtime::configure(config.device);
        crate::platform::set_orientation(config.orientation);
        crate::native_ime::configure_screen(config.screen.immersive, config.screen.avoid_cutout);
        if launch.render_scale > 0 {
            config.render_scale = crate::display::RenderScale::new(launch.render_scale);
        }
        crate::display::set_scale(config.render_scale);
        horizon_client::android_resources::repair_structure_transforms(
            &home.join("engine-data.zip"),
            &launch.resources(),
        )?;
        if config.record_performance || config.device.gpu_profile {
            crate::performance::start(
                &home,
                serde_json::json!({
                    "host": "cs-launcher-plus",
                    "renderer": renderer.label(),
                    "resolution_percent": config.render_scale.percent(),
                    "source_commit": option_env!("HORIZON_ANDROID_BUILD_COMMIT"),
                }),
            )?;
        }
        crate::diagnostics::heartbeat("cs_play_requested", || {
            serde_json::json!({"renderer": renderer.label(), "resolution_percent": config.render_scale.percent()})
        });
        let session_file = (!launch.session_file.is_empty()).then(|| PathBuf::from(&launch.session_file));
        let request = crate::game_launch::LaunchRequest {
            resources: launch.resources(),
            game_home: game_home.clone(),
            session_file,
            offline_name: launch.offline_name.clone(),
            server: launch.server.clone(),
            control_socket: None,
            extra_arguments: launch.extra_arguments.clone(),
            open_gl: renderer == crate::catalog::RendererMode::OpenGl,
            gpu_batching: renderer == crate::catalog::RendererMode::Vulkan,
        };
        let game = crate::game_launch::start(request)?;
        let mut host = Host {
            game,
            home: home.clone(),
            immersive: config.screen.immersive,
            avoid_cutout: config.screen.avoid_cutout,
            aspect: config.screen.aspect.ratio(),
        };
        crate::android_event_loop()?.run_app(&mut host)?;
        Ok(())
    })();

    match result {
        Ok(()) => write_exit(&home, None),
        Err(error) => {
            log::error!("Horizon (Masterix host): {error}");
            crate::diagnostics::record_error(&error.to_string());
            horizon_client::crash::report_error(&home, "client", &*error);
            let _ = std::fs::write(home.join("android-error.txt"), error.to_string());
            write_exit(&home, Some(&error.to_string()));
        }
    }
    crate::performance::stop();
    crate::diagnostics::finish();
    crate::platform::finish();
    crate::APP.with(|slot| *slot.borrow_mut() = None);
    std::process::exit(0);
}

/// The game as the only handler. The Rust launcher's `restore_launcher` becomes "leave the activity".
struct Host {
    game: Box<dyn ApplicationHandler>,
    home: PathBuf,
    immersive: bool,
    avoid_cutout: bool,
    aspect: [u32; 2],
}

impl Host {
    fn leave(&mut self, events: &ActiveEventLoop) {
        self.game.exiting(events);
        crate::performance::stop();
        crate::platform::hide();
        horizon_client::auth::clear_current();
        horizon_client::discord_presence::stop();
        crate::diagnostics::heartbeat("cs_left_game", || serde_json::json!({}));
        if let Ok(message) = std::fs::read_to_string(self.home.join("android-error.txt")) {
            log::error!("Horizon ended with: {message}");
        }
        events.exit();
    }
}

impl ApplicationHandler for Host {
    fn new_events(&mut self, events: &ActiveEventLoop, cause: StartCause) {
        self.game.new_events(events, cause);
    }
    fn resumed(&mut self, events: &ActiveEventLoop) {
        crate::native_ime::configure_screen(self.immersive, self.avoid_cutout);
        winit::platform::android::set_native_content_aspect(self.aspect);
        self.game.resumed(events);
    }
    fn suspended(&mut self, events: &ActiveEventLoop) {
        crate::diagnostics::heartbeat("suspended", || serde_json::json!({}));
        self.game.suspended(events);
    }
    fn window_event(&mut self, events: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        self.game.window_event(events, id, event);
    }
    fn device_event(&mut self, events: &ActiveEventLoop, id: DeviceId, event: DeviceEvent) {
        self.game.device_event(events, id, event);
    }
    fn about_to_wait(&mut self, events: &ActiveEventLoop) {
        self.game.about_to_wait(events);
        if horizon_client::android_runtime::take_launcher_return() {
            self.leave(events);
        }
    }
    fn memory_warning(&mut self, events: &ActiveEventLoop) {
        self.game.memory_warning(events);
    }
    fn exiting(&mut self, events: &ActiveEventLoop) {
        self.game.exiting(events);
    }
}

// ---------------------------------------------------------------------------
// JNI: install / query without a window (called from a CS foreground service)
// ---------------------------------------------------------------------------

fn jstring_arg(env: &mut JNIEnv<'_>, value: &JString<'_>) -> String {
    if value.is_null() {
        return String::new();
    }
    env.get_string(value).map(Into::into).unwrap_or_default()
}

fn jstring_result(env: &mut JNIEnv<'_>, text: Option<String>) -> jstring {
    match text {
        Some(text) => env
            .new_string(text)
            .map(|s| s.into_raw())
            .unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

/// `String nativeInstall(String home, String cache, String engineData, InstallListener listener)`
/// Returns `null` on success, otherwise the error text. `listener.onProgress(String stage, long done, long total)`
/// is called on this thread (at least once a second while the installer is alive); `listener.isCancelled()` is
/// polled every second and stops the installer before the next download or stage.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_engine_horizon_HorizonNative_nativeInstall<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    home: JString<'l>,
    cache: JString<'l>,
    engine_data: JString<'l>,
    listener: JObject<'l>,
) -> jstring {
    crate::init_logging();
    let home = PathBuf::from(jstring_arg(&mut env, &home));
    let cache = PathBuf::from(jstring_arg(&mut env, &cache));
    let engine_data = jstring_arg(&mut env, &engine_data);
    if home.as_os_str().is_empty() || cache.as_os_str().is_empty() {
        return jstring_result(&mut env, Some("home and cache are required".into()));
    }
    if let Err(error) = std::fs::create_dir_all(&home).and_then(|()| std::fs::create_dir_all(&cache)) {
        return jstring_result(&mut env, Some(format!("Cannot create engine folders: {error}")));
    }
    if let Err(error) = stage_engine_data(&engine_data, &home) {
        return jstring_result(&mut env, Some(format!("Cannot stage engine data: {error}")));
    }
    if !home.join("engine-data.zip").is_file() {
        return jstring_result(&mut env, Some("engine-data.zip is missing".into()));
    }
    crate::backend::initialize(home.clone());
    horizon_client::android_runtime::configure_cache_dir(cache.clone());
    horizon_client::official_install::configure_download_cache_dir(cache);

    enum Message {
        Progress(horizon_client::official_install::InstallProgress),
        Done(Result<(), String>),
    }
    let (sender, receiver) = mpsc::channel::<Message>();
    let worker_home = home.clone();
    let progress_sender = sender.clone();
    std::thread::Builder::new()
        .name("cs-horizon-install".into())
        .spawn(move || {
            let result = horizon_client::official_install::install_with_progress(&worker_home, |update| {
                let _ = progress_sender.send(Message::Progress(update));
            })
            .map_err(|e| e.to_string());
            let _ = sender.send(Message::Done(result));
        })
        .expect("spawn install thread");

    let mut last = (String::new(), 0u64, 0u64);
    let mut outcome: Result<(), String> = Ok(());
    let report_to_java = |env: &mut JNIEnv<'l>, stage: &str, done: u64, total: u64| {
        if listener.is_null() {
            return;
        }
        let _ = env.with_local_frame(8, |env| -> jni::errors::Result<()> {
            let text = env.new_string(stage)?;
            env.call_method(
                &listener,
                "onProgress",
                "(Ljava/lang/String;JJ)V",
                &[
                    JValue::Object(&text),
                    JValue::Long(done as i64),
                    JValue::Long(total as i64),
                ],
            )?;
            Ok(())
        });
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
        }
    };
    let mut last_java_update = std::time::Instant::now();
    loop {
        match receiver.recv_timeout(std::time::Duration::from_millis(1000)) {
            Ok(Message::Progress(update)) => {
                let stage = update.stage.label().to_string();
                let same_stage = stage == last.0;
                let done = if same_stage { update.done.max(last.1) } else { update.done };
                if same_stage && done == last.1 && update.total == last.2 {
                    continue;
                }
                last = (stage.clone(), done, update.total);
                last_java_update = std::time::Instant::now();
                report_to_java(&mut env, &stage, done, update.total);
            }
            Ok(Message::Done(result)) => {
                outcome = result;
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Heartbeat: long silent stages (unpacking, world data) still prove the installer is alive,
                // and the host gets a chance to cancel.
                if !listener.is_null() {
                    let cancelled = env
                        .call_method(&listener, "isCancelled", "()Z", &[])
                        .and_then(|v| v.z())
                        .unwrap_or(false);
                    if env.exception_check().unwrap_or(false) {
                        let _ = env.exception_clear();
                    }
                    if cancelled {
                        horizon_client::official_install::CANCEL_REQUESTED
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                if last_java_update.elapsed() >= std::time::Duration::from_millis(1000) && !last.0.is_empty() {
                    last_java_update = std::time::Instant::now();
                    report_to_java(&mut env, &last.0, last.1, last.2);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                outcome = Err("Installer thread stopped unexpectedly".into());
                break;
            }
        }
    }
    if outcome.is_ok() {
        if let Err(error) = horizon_client::official_install::cleanup_download_cache(&home) {
            log::warn!("Could not clean the Minecraft download cache: {error}");
        }
    }
    jstring_result(&mut env, outcome.err())
}

/// `boolean nativeReady(String home)` — the prepared resources and sounds are complete.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_engine_horizon_HorizonNative_nativeReady<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    home: JString<'l>,
) -> jboolean {
    let home = PathBuf::from(jstring_arg(&mut env, &home));
    u8::from(!home.as_os_str().is_empty() && horizon_client::official_install::ready(&home))
}

/// `String nativeVersion()` — `"<engine version>;<minecraft version>;<build commit>"`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_engine_horizon_HorizonNative_nativeVersion<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
) -> jstring {
    let text = format!(
        "{};{};{}",
        crate::backend::VERSION,
        horizon_core::VANILLA_VERSION,
        option_env!("HORIZON_ANDROID_BUILD_COMMIT").unwrap_or("unknown")
    );
    jstring_result(&mut env, Some(text))
}

/// `String nativeDiagnosticsSummary(String home)` — the launcher's one-line summary of the previous session.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_engine_horizon_HorizonNative_nativeDiagnosticsSummary<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    home: JString<'l>,
) -> jstring {
    let home = PathBuf::from(jstring_arg(&mut env, &home));
    if home.as_os_str().is_empty() {
        return jstring_result(&mut env, None);
    }
    let summary = crate::diagnostics::summary(&home);
    jstring_result(&mut env, (!summary.is_empty()).then_some(summary))
}

// ---------------------------------------------------------------------------
// Host input natives (CS control system -> engine). Cheap: they only queue.
// ---------------------------------------------------------------------------
use horizon_client::android_runtime::{host_push, HostEvent};

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_engine_horizon_HorizonNative_nativeHostKey<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    token: JString<'l>,
    down: jboolean,
) {
    let token = jstring_arg(&mut env, &token);
    if !token.is_empty() {
        host_push(HostEvent::Key { token, down: down != 0 });
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_engine_horizon_HorizonNative_nativeHostLook<'l>(
    _env: JNIEnv<'l>,
    _class: JClass<'l>,
    dx: jni::sys::jdouble,
    dy: jni::sys::jdouble,
) {
    if dx != 0.0 || dy != 0.0 {
        host_push(HostEvent::Look { dx, dy });
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_engine_horizon_HorizonNative_nativeHostCursor<'l>(
    _env: JNIEnv<'l>,
    _class: JClass<'l>,
    x: jni::sys::jdouble,
    y: jni::sys::jdouble,
) {
    host_push(HostEvent::Cursor { x, y });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_engine_horizon_HorizonNative_nativeHostButton<'l>(
    _env: JNIEnv<'l>,
    _class: JClass<'l>,
    button: jni::sys::jint,
    down: jboolean,
) {
    host_push(HostEvent::Button {
        button: button.clamp(0, 255) as u8,
        down: down != 0,
    });
}

/// A touch began at (x, y) window pixels while the world has the pointer: if it lies on
/// the hotbar drawn last frame, the engine selects that slot and the slot index (0..8)
/// comes back so the launcher swallows the touch; -1 otherwise.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_engine_horizon_HorizonNative_nativeHostHotbarTap<'l>(
    _env: JNIEnv<'l>,
    _class: JClass<'l>,
    x: jni::sys::jdouble,
    y: jni::sys::jdouble,
) -> jni::sys::jint {
    let Some([rx, ry, rw, rh]) = horizon_client::android_runtime::host_hotbar() else {
        return -1;
    };
    if rw <= 0.0 || rh <= 0.0 || x < rx || x >= rx + rw || y < ry || y >= ry + rh {
        return -1;
    }
    host_push(HostEvent::HotbarTap { x, y });
    (((x - rx) / rw * 9.0) as i32).clamp(0, 8)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_engine_horizon_HorizonNative_nativeHostScroll<'l>(
    _env: JNIEnv<'l>,
    _class: JClass<'l>,
    x: jni::sys::jfloat,
    y: jni::sys::jfloat,
) {
    host_push(HostEvent::Scroll { x, y });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_engine_horizon_HorizonNative_nativeHostText<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    text: JString<'l>,
) {
    let text = jstring_arg(&mut env, &text);
    if !text.is_empty() {
        host_push(HostEvent::Text(text));
    }
}

/// Skin & Cape changed in the host: PNG paths (empty = none) and `slim`/`classic`.
/// CS voice chat: the host's microphone samples (16 kHz mono PCM16).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_engine_horizon_HorizonNative_nativeHostMic<'l>(
    env: JNIEnv<'l>,
    _class: JClass<'l>,
    data: jni::objects::JShortArray<'l>,
    count: jni::sys::jint,
) {
    let count = count.max(0) as usize;
    let mut buf = vec![0i16; count];
    if count > 0 && env.get_short_array_region(&data, 0, &mut buf).is_ok() {
        horizon_client::android_runtime::host_mic_push(&buf);
    }
}
/// CS voice chat: the host's microphone state (0 idle, 1 recording, 2 failed).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_engine_horizon_HorizonNative_nativeHostMicState<'l>(
    _env: JNIEnv<'l>,
    _class: JClass<'l>,
    state: jni::sys::jint,
) {
    horizon_client::android_runtime::host_mic_set_state(state.clamp(0, 2) as u8);
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_engine_horizon_HorizonNative_nativeHostSkin<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    skin: JString<'l>,
    model: JString<'l>,
    cape: JString<'l>,
) {
    let skin = jstring_arg(&mut env, &skin);
    let model = jstring_arg(&mut env, &model);
    let cape = jstring_arg(&mut env, &cape);
    host_push(HostEvent::Skin { skin, model, cape });
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_engine_horizon_HorizonNative_nativeHostReleaseAll<'l>(
    _env: JNIEnv<'l>,
    _class: JClass<'l>,
) {
    host_push(HostEvent::ReleaseAll);
}

/// The world's public e4mc address while it is published, else null: the launcher's
/// Play with Friends invites carry it.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_engine_horizon_HorizonNative_nativeHostPublished<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
) -> jstring {
    jstring_result(&mut env, horizon_client::android_runtime::host_published())
}

/// The next CS Friends notice (`title\ntext`) for a system notification, else null.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_engine_horizon_HorizonNative_nativeHostNotice<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
) -> jstring {
    jstring_result(&mut env, horizon_client::android_runtime::host_take_notice())
}

/// What the player is doing (`scene\tworld\tserver\tplayers`), for Discord Rich Presence and play time.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_engine_horizon_HorizonNative_nativeHostStatus<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
) -> jstring {
    jstring_result(&mut env, Some(horizon_client::android_runtime::host_status()))
}

/// Bit 0: the game wants the pointer captured (world view); bit 1: it asked for a text keyboard.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_engine_horizon_HorizonNative_nativeHostPoll<'l>(
    _env: JNIEnv<'l>,
    _class: JClass<'l>,
) -> jni::sys::jint {
    let mut flags = 0;
    if horizon_client::android_runtime::host_grab() {
        flags |= 1;
    }
    if horizon_client::android_runtime::host_keyboard() {
        flags |= 2;
    }
    if horizon_client::android_runtime::host_take_skin_request() {
        flags |= 4;
    }
    if horizon_client::android_runtime::host_take_mic_request() {
        flags |= 8;
    }
    if horizon_client::android_runtime::host_mic_wanted() {
        flags |= 16;
    }
    flags
}
