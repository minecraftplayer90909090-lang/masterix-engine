//! CS Launcher Plus: the Java edition's world through e4mc.
//!
//! The launcher watches the game log for `Local game hosted on port N`
//! (Open to LAN) and publishes that loopback port with the same
//! `horizon-e4mc` session CS Engine uses, so friends join the Java world at
//! `<name>.e4mc.link` exactly like a CS Engine world. One handle per
//! publish; the launcher polls `domain` / `guests` / `problem` and stops it
//! when the game exits.
//!
//! Java side: `com.craftstudio.cslauncher.e4mc.E4mcNative`.
use jni::JNIEnv;
use jni::objects::{JClass, JString};
use jni::sys::{jint, jlong, jstring};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Mutex;

static LAST_ERROR: Mutex<String> = Mutex::new(String::new());

fn set_error(text: String) {
    *LAST_ERROR.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = text;
}

fn string_out(env: &mut JNIEnv, text: Option<String>) -> jstring {
    match text {
        Some(text) => env
            .new_string(text)
            .map(|s| s.into_raw())
            .unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

/// The publish behind a handle; `0` is no handle.
fn publish<'a>(handle: jlong) -> Option<&'a horizon_e4mc::Publish> {
    if handle == 0 {
        return None;
    }
    // SAFETY: handles come from `nativeStart` (a leaked Box) and are only
    // freed by `nativeStop`, after which the launcher drops them.
    Some(unsafe { &*(handle as *const horizon_e4mc::Publish) })
}

/// Start publishing the LAN world on `port` (loopback). Returns the handle,
/// or 0 (see `nativeError`).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_e4mc_E4mcNative_nativeStart(
    mut env: JNIEnv,
    _class: JClass,
    port: jint,
    relay: JString,
) -> jlong {
    android_logger::init_once(
        android_logger::Config::default()
            .with_max_level(log::LevelFilter::Info)
            .with_tag("cs-e4mc"),
    );
    let Ok(port) = u16::try_from(port) else {
        set_error(format!("bad LAN port {port}"));
        return 0;
    };
    let relay = if relay.is_null() {
        None
    } else {
        env.get_string(&relay).ok().and_then(|s| {
            let s: String = s.into();
            let (host, port) = s.rsplit_once(':')?;
            Some((host.to_owned(), port.parse().ok()?))
        })
    };
    let config = horizon_e4mc::PublishConfig {
        lan: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port),
        relay,
    };
    match horizon_e4mc::Publish::start(config) {
        Ok(publish) => Box::into_raw(Box::new(publish)) as jlong,
        Err(e) => {
            set_error(e);
            0
        }
    }
}

/// The world's public address (`<name>.e4mc.link`) once the relay assigned it, else null.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_e4mc_E4mcNative_nativeDomain(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jstring {
    let domain = publish(handle).and_then(|p| p.state().domain);
    string_out(&mut env, domain)
}

/// Why publishing isn't serving (text for the player), else null.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_e4mc_E4mcNative_nativeProblem(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jstring {
    let problem = publish(handle).and_then(|p| p.state().problem);
    string_out(&mut env, problem)
}

/// Guests being carried right now.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_e4mc_E4mcNative_nativeGuests(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jint {
    publish(handle).map_or(0, |p| p.sessions() as jint)
}

/// Who the relay lets in from now on (CS Launcher's World hosting settings):
/// `max_players` guests at once (0: no limit here), and, when `allowed` is
/// not null, only the names it lists (newline separated), others told
/// `reason`. Guests already playing stay.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_e4mc_E4mcNative_nativeSetPolicy(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    max_players: jint,
    allowed: JString,
    reason: JString,
) {
    let Some(publish) = publish(handle) else {
        return;
    };
    let mut text = |s: &JString| -> Option<String> {
        if s.is_null() {
            None
        } else {
            env.get_string(s).ok().map(Into::into)
        }
    };
    let allowed = text(&allowed).map(|list| {
        list.lines()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .collect()
    });
    let closed_reason = text(&reason).unwrap_or_default();
    publish.set_policy(horizon_e4mc::GuestPolicy {
        max_players: usize::try_from(max_players).ok().filter(|&n| n > 0),
        allowed,
        closed_reason,
    });
}

/// What happened since the last poll that the player should hear of: one
/// line per refused guest (`name\treason`), newline separated; null when nothing.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_e4mc_E4mcNative_nativePoll(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jstring {
    let lines: Vec<String> = publish(handle)
        .map(|p| {
            p.poll()
                .into_iter()
                .filter_map(|e| match e {
                    horizon_e4mc::PublishEvent::GuestRefused { name, reason } => {
                        Some(format!("{name}\t{reason}"))
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    string_out(&mut env, (!lines.is_empty()).then(|| lines.join("\n")))
}

/// The error of the last failed `nativeStart`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_e4mc_E4mcNative_nativeError(
    mut env: JNIEnv,
    _class: JClass,
) -> jstring {
    let text = LAST_ERROR
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    string_out(&mut env, Some(text))
}

/// End the publish and free the handle (the domain stops answering, guests drop).
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_craftstudio_cslauncher_e4mc_E4mcNative_nativeStop(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle == 0 {
        return;
    }
    // SAFETY: the handle came from `nativeStart` and is dropped exactly once here.
    let publish = unsafe { Box::from_raw(handle as *mut horizon_e4mc::Publish) };
    publish.stop();
}
