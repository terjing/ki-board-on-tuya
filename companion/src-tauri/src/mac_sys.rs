//! macOS system bindings: CGEvent injection, Accessibility (AX) window focus,
//! TCC permission checks and the keyboard-layout cache.
//!
//! Threading rules (plan D4, binding on every implementer):
//! * TIS (`TISCopyCurrentKeyboardLayoutInputSource` & co.) is main-thread-only
//!   and runs ONLY inside [`refresh_layout_cache`] and the main-thread branch
//!   of [`keycode_for_char`] (plus the layout-changed notification callback,
//!   which is delivered on the main run loop and calls the former).
//! * Off-main callers read the layout cache only; they never dispatch to, or
//!   wait on, the main thread.
//! * `LAYOUT_CACHE` is a leaf lock: never held while acquiring another lock and
//!   never held across a TIS call.
//! * No function here takes or waits on an app lock, the main queue or the
//!   Tauri runtime.
//! * AX calls block up to the messaging timeout (0.25 s) per unresponsive app.
//!
//! Layout-change hardening (plan review r3, n1): the first
//! [`refresh_layout_cache`] call on the main thread registers an observer for
//! the distributed notification `kTISNotifySelectedKeyboardInputSourceChanged`.
//! When it fires (main run loop) the cache is CLEARED first — so off-main
//! lookups fail closed instead of sending a stale keycode (e.g. ⌘+0x00 = ⌘Q
//! on AZERTY) — and then recomputed on the main thread.
//!
//! Every `extern` below names the Apple header it was checked against. None of
//! this links on the Linux dev host: the darwin cross-checks only type-check
//! (plan §7 risk 1).

use std::collections::HashMap;
use std::ffi::c_void;
use std::os::raw::c_int;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::thread::sleep;
use std::time::{Duration, Instant};

use core_foundation::array::CFArray;
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::boolean::CFBoolean;
use core_foundation::data::{CFData, CFDataRef};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::{CFNumber, CFNumberRef};
use core_foundation::string::{CFString, CFStringRef};
use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGEventType};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use core_graphics::window::{
    copy_window_info, kCGNullWindowID, kCGWindowLayer, kCGWindowListExcludeDesktopElements,
    kCGWindowListOptionAll, kCGWindowListOptionOnScreenOnly, kCGWindowOwnerPID,
};

use crate::keymap::{self, DictationMenuItem, KeyStroke, MacKey};

// ───────────────────────────── Raw FFI ─────────────────────────────

type AXError = i32;
type AXUIElementRef = CFTypeRef;
type TISInputSourceRef = CFTypeRef;
type OSStatus = i32;

/// `kAXErrorSuccess` (AXError.h).
const AX_SUCCESS: AXError = 0;

// ApplicationServices/HIServices/AXUIElement.h, AXError.h, AXValue.h.
// Checked against: AXUIElement.h (`AXUIElementCreateApplication(pid_t)`,
// `AXUIElementCreateSystemWide()`, `AXUIElementCopyAttributeValue(AXUIElementRef,
// CFStringRef, CFTypeRef*)`, `AXUIElementSetAttributeValue(…, CFTypeRef)`,
// `AXUIElementPerformAction(…, CFStringRef)`, `AXUIElementGetPid(…, pid_t*)`,
// `AXUIElementSetMessagingTimeout(…, float)`), AXUIElement.h /
// AXIsProcessTrusted: `Boolean AXIsProcessTrusted(void)`,
// `Boolean AXIsProcessTrustedWithOptions(CFDictionaryRef)`,
// `extern const CFStringRef kAXTrustedCheckOptionPrompt`.
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: *mut CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: CFStringRef,
        value: CFTypeRef,
    ) -> AXError;
    fn AXUIElementPerformAction(element: AXUIElementRef, action: CFStringRef) -> AXError;
    fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> AXError;
    fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout_secs: f32) -> AXError;
    fn AXIsProcessTrusted() -> u8;
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> u8;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}

// CoreGraphics/CGEventSource.h (macOS 10.15+): `bool CGPreflightPostEventAccess(void)`.
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightPostEventAccess() -> bool;
}

// Carbon.framework → HIToolbox/TextInputSources.h and Events/UnicodeUtilities.h.
// `TISInputSourceRef TISCopyCurrentKeyboardLayoutInputSource(void)` (Copy rule),
// `TISCopyCurrentASCIICapableKeyboardLayoutInputSource(void)` (Copy rule),
// `void* TISGetInputSourceProperty(TISInputSourceRef, CFStringRef)` (Get rule),
// `extern const CFStringRef kTISPropertyUnicodeKeyLayoutData` (CFDataRef value),
// `extern const CFStringRef kTISNotifySelectedKeyboardInputSourceChanged`.
// CarbonCore/LowMem.h: `UInt8 LMGetKbdType(void)`.
// UnicodeUtilities.h: `OSStatus UCKeyTranslate(const UCKeyboardLayout*, UInt16
// virtualKeyCode, UInt16 keyAction, UInt32 modifierKeyState, UInt32 keyboardType,
// OptionBits keyTranslateOptions, UInt32* deadKeyState, UniCharCount
// maxStringLength, UniCharCount* actualStringLength, UniChar unicodeString[])`.
#[link(name = "Carbon", kind = "framework")]
extern "C" {
    fn TISCopyCurrentKeyboardLayoutInputSource() -> TISInputSourceRef;
    fn TISCopyCurrentASCIICapableKeyboardLayoutInputSource() -> TISInputSourceRef;
    fn TISGetInputSourceProperty(source: TISInputSourceRef, key: CFStringRef) -> *const c_void;
    static kTISPropertyUnicodeKeyLayoutData: CFStringRef;
    static kTISNotifySelectedKeyboardInputSourceChanged: CFStringRef;
    fn LMGetKbdType() -> u8;
    fn UCKeyTranslate(
        key_layout_ptr: *const c_void,
        virtual_key_code: u16,
        key_action: u16,
        modifier_key_state: u32,
        keyboard_type: u32,
        key_translate_options: u32,
        dead_key_state: *mut u32,
        max_string_length: usize,
        actual_string_length: *mut usize,
        unicode_string: *mut u16,
    ) -> OSStatus;
}

// CoreFoundation/CFNotificationCenter.h: `CFNotificationCenterGetDistributedCenter(void)`,
// `CFNotificationCenterAddObserver(center, const void *observer, CFNotificationCallback,
// CFStringRef name, const void *object, CFNotificationSuspensionBehavior)`;
// `CFNotificationCallback = void (*)(CFNotificationCenterRef, void *observer,
// CFNotificationName, const void *object, CFDictionaryRef userInfo)`;
// `CFNotificationSuspensionBehaviorDeliverImmediately = 4`.
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFNotificationCenterGetDistributedCenter() -> *const c_void;
    fn CFNotificationCenterAddObserver(
        center: *const c_void,
        observer: *const c_void,
        callback: extern "C" fn(*const c_void, *mut c_void, CFStringRef, *const c_void, CFDictionaryRef),
        name: CFStringRef,
        object: *const c_void,
        suspension_behavior: isize,
    );
}
const CF_NOTIFY_DELIVER_IMMEDIATELY: isize = 4;

// pthread.h (libSystem, declared in <pthread.h> on Darwin): `int pthread_main_np(void)`.
// This is the ONLY libSystem symbol used; no main-queue dispatch (plan D4 / r2 N1).
extern "C" {
    fn pthread_main_np() -> c_int;
}

/// UnicodeUtilities.h: `kUCKeyActionDisplay`.
const UC_KEY_ACTION_DISPLAY: u16 = 3;
/// UnicodeUtilities.h: `kUCKeyTranslateNoDeadKeysMask` (1 << kUCKeyTranslateNoDeadKeysBit).
const UC_KEY_TRANSLATE_NO_DEAD_KEYS: u32 = 1;
/// Events.h: `cmdKey` (0x0100); `UCKeyTranslate` takes `(modifiers >> 8) & 0xFF`.
const CMD_KEY_STATE: u32 = 0x0100 >> 8;

/// How long AX may block on one unresponsive application (plan D3).
const AX_MESSAGING_TIMEOUT_SECS: f32 = 0.25;
/// Frontmost poll after asking for activation (plan D3).
const FRONTMOST_POLL: Duration = Duration::from_millis(200);
const FRONTMOST_POLL_STEP: Duration = Duration::from_millis(10);
const FOCUS_SETTLE: Duration = Duration::from_millis(40);
/// Pause between Unicode chunks (plan D1).
const CHUNK_PAUSE: Duration = Duration::from_millis(3);
/// Gap between the two taps of a double-modifier trigger (plan D5).
const DOUBLE_TAP_GAP: Duration = Duration::from_millis(90);

/// `focus_window_by_title` error prefix for "nothing matches"; callers
/// (`kirocrew_control`) use it to tell a miss from a focus failure.
pub(crate) const ERR_NO_MATCH: &str = "no window matches title";

// ───────────────────────────── Permission ─────────────────────────────

/// `AXIsProcessTrusted()` — the single grant covering AX focus and posting.
pub fn input_permission_granted() -> bool {
    unsafe { AXIsProcessTrusted() != 0 }
}

/// `CGPreflightPostEventAccess()` — diagnostic only.
pub fn post_event_access_preflight() -> bool {
    unsafe { CGPreflightPostEventAccess() }
}

/// Show the one system Accessibility prompt; returns the current grant.
/// Deliberately does NOT call `CGRequestPostEventAccess` (plan D2).
pub fn request_input_permission() -> bool {
    unsafe {
        let key = CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt);
        let opts = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), CFBoolean::true_value().as_CFType())]);
        AXIsProcessTrustedWithOptions(opts.as_concrete_TypeRef()) != 0
    }
}

// ─────────────────────────── Layout cache ───────────────────────────

/// Leaf lock (module docs): never held across TIS or while taking any other lock.
static LAYOUT_CACHE: Mutex<Option<HashMap<(char, bool), u16>>> = Mutex::new(None);
/// Set once the layout-changed observer is registered. Its address doubles as
/// the (never removed) observer identity.
static LAYOUT_OBSERVER_REGISTERED: AtomicBool = AtomicBool::new(false);

fn on_main_thread() -> bool {
    unsafe { pthread_main_np() != 0 }
}

fn cache_replace(map: HashMap<(char, bool), u16>) {
    let mut g = LAYOUT_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    *g = Some(map);
}

fn cache_clear() {
    let mut g = LAYOUT_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    *g = None;
}

fn cache_get(c: char, with_cmd: bool) -> Option<u16> {
    let g = LAYOUT_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    g.as_ref().and_then(|m| m.get(&(c, with_cmd)).copied())
}

fn cache_set(c: char, with_cmd: bool, code: Option<u16>) {
    let mut g = LAYOUT_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let m = g.get_or_insert_with(HashMap::new);
    match code {
        Some(k) => {
            m.insert((c, with_cmd), k);
        }
        None => {
            m.remove(&(c, with_cmd));
        }
    }
}

/// `kTISNotifySelectedKeyboardInputSourceChanged` callback. Delivered on the
/// main run loop (registered from the main thread). Clears the cache first so
/// off-main lookups fail closed, then recomputes on this (main) thread.
extern "C" fn on_input_source_changed(
    _center: *const c_void,
    _observer: *mut c_void,
    _name: CFStringRef,
    _object: *const c_void,
    _user_info: CFDictionaryRef,
) {
    cache_clear();
    let _ = refresh_layout_cache();
}

/// Register the layout-changed observer once. Main thread only; private,
/// called from the refresh paths.
fn ensure_layout_observer() {
    if !on_main_thread() {
        return;
    }
    if LAYOUT_OBSERVER_REGISTERED.swap(true, Ordering::AcqRel) {
        return;
    }
    unsafe {
        CFNotificationCenterAddObserver(
            CFNotificationCenterGetDistributedCenter(),
            &LAYOUT_OBSERVER_REGISTERED as *const AtomicBool as *const c_void,
            on_input_source_changed,
            kTISNotifySelectedKeyboardInputSourceChanged,
            std::ptr::null(),
            CF_NOTIFY_DELIVER_IMMEDIATELY,
        );
    }
}

/// Current keyboard layout source (Copy rule). `ascii_capable` selects the
/// ASCII-capable fallback used when the current source has no layout data
/// (e.g. an IME such as Pinyin).
fn copy_layout_source(ascii_capable: bool) -> Option<CFType> {
    unsafe {
        let p = if ascii_capable {
            TISCopyCurrentASCIICapableKeyboardLayoutInputSource()
        } else {
            TISCopyCurrentKeyboardLayoutInputSource()
        };
        if p.is_null() {
            None
        } else {
            Some(CFType::wrap_under_create_rule(p))
        }
    }
}

/// Resolve `c` (as typed with / without ⌘) to a virtual keycode through TIS +
/// `UCKeyTranslate`. MAIN THREAD ONLY.
fn tis_lookup(c: char, with_cmd: bool) -> Option<u16> {
    debug_assert!(on_main_thread());
    let want = c.to_lowercase().next()?;
    unsafe {
        let mut source = copy_layout_source(false);
        let mut data = source
            .as_ref()
            .map(|s| TISGetInputSourceProperty(s.as_CFTypeRef(), kTISPropertyUnicodeKeyLayoutData))
            .unwrap_or(std::ptr::null());
        if data.is_null() {
            source = copy_layout_source(true);
            data = source
                .as_ref()
                .map(|s| TISGetInputSourceProperty(s.as_CFTypeRef(), kTISPropertyUnicodeKeyLayoutData))
                .unwrap_or(std::ptr::null());
        }
        if data.is_null() {
            return None;
        }
        // Get rule: wrap_under_get_rule retains, so the bytes outlive `source`'s use.
        let cf_data = CFData::wrap_under_get_rule(data as CFDataRef);
        let layout = cf_data.bytes().as_ptr() as *const c_void;
        let kbd_type = LMGetKbdType() as u32;
        let modifiers = if with_cmd { CMD_KEY_STATE } else { 0 };
        for keycode in 0u16..128 {
            let mut dead: u32 = 0;
            let mut len: usize = 0;
            let mut buf = [0u16; 4];
            let status = UCKeyTranslate(
                layout,
                keycode,
                UC_KEY_ACTION_DISPLAY,
                modifiers,
                kbd_type,
                UC_KEY_TRANSLATE_NO_DEAD_KEYS,
                &mut dead,
                buf.len(),
                &mut len,
                buf.as_mut_ptr(),
            );
            if status != 0 || len != 1 {
                continue;
            }
            let got = char::decode_utf16(buf[..1].iter().copied()).next();
            if let Some(Ok(ch)) = got {
                if ch.to_lowercase().next() == Some(want) {
                    drop(source);
                    return Some(keycode);
                }
            }
        }
        None
    }
}

/// Main thread only (no-op returning false elsewhere). Never takes an app lock.
pub fn refresh_layout_cache() -> bool {
    if !on_main_thread() {
        return false;
    }
    ensure_layout_observer();
    let mut map = HashMap::new();
    for &(c, with_cmd) in keymap::LAYOUT_CACHE_KEYS {
        if let Some(k) = tis_lookup(c, with_cmd) {
            map.insert((c, with_cmd), k);
        }
    }
    let ok = map.len() == keymap::LAYOUT_CACHE_KEYS.len();
    // An unresolved key is absent from `map`, so the swap removes it (no stale entry).
    cache_replace(map);
    ok
}

/// Main thread: TIS direct + cache update. Off main: cache read only; `None` =
/// fail closed.
pub fn keycode_for_char(c: char, with_cmd: bool) -> Option<u16> {
    if on_main_thread() {
        ensure_layout_observer();
        let found = tis_lookup(c, with_cmd);
        cache_set(c, with_cmd, found);
        found
    } else {
        cache_get(c, with_cmd)
    }
}

// ───────────────────────────── CGEvent posting ─────────────────────────────

/// One event plus the pause to take after posting it.
struct Planned {
    event: CGEvent,
    pause_after: Duration,
}

fn hid_source() -> Result<CGEventSource, String> {
    CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|_| "CGEventSource creation failed — nothing injected".to_string())
}

fn key_event(
    src: &CGEventSource,
    keycode: u16,
    down: bool,
    flags: u64,
    ty: Option<CGEventType>,
) -> Result<CGEvent, String> {
    let ev = CGEvent::new_keyboard_event(src.clone(), keycode, down)
        .map_err(|_| format!("CGEvent creation failed for keycode 0x{keycode:02X} — nothing injected"))?;
    if let Some(ty) = ty {
        ev.set_type(ty);
    }
    // Always set flags explicitly: events from the HID source otherwise inherit
    // whatever modifiers the user is physically holding.
    ev.set_flags(CGEventFlags::from_bits_retain(flags));
    Ok(ev)
}

fn require_permission() -> Result<(), String> {
    if input_permission_granted() {
        Ok(())
    } else {
        Err("Accessibility permission not granted — nothing injected".to_string())
    }
}

/// Post an already-built sequence. Creation (the only fallible step) is done;
/// this cannot leave a half-built stroke behind.
fn post_planned(events: Vec<Planned>) {
    for p in events {
        p.event.post(CGEventTapLocation::HID);
        if !p.pause_after.is_zero() {
            sleep(p.pause_after);
        }
    }
}

/// Plan the events for one stroke with an already-resolved keycode.
fn plan_stroke(src: &CGEventSource, keycode: u16, stroke: &KeyStroke, out: &mut Vec<Planned>) -> Result<(), String> {
    let mods = stroke.mods.modifier_keycodes();
    let flags = stroke.mods.cg_flags();
    let push = |out: &mut Vec<Planned>, ev: CGEvent| out.push(Planned { event: ev, pause_after: Duration::ZERO });
    for (kc, acc) in &mods {
        push(out, key_event(src, *kc, true, *acc, None)?);
    }
    push(out, key_event(src, keycode, true, flags, None)?);
    push(out, key_event(src, keycode, false, flags, None)?);
    for i in (0..mods.len()).rev() {
        let remaining = if i == 0 { 0 } else { mods[i - 1].1 };
        push(out, key_event(src, mods[i].0, false, remaining, None)?);
    }
    Ok(())
}

pub fn post_strokes(strokes: &[KeyStroke]) -> Result<(), String> {
    require_permission()?;
    // 1. Resolve every Char key BEFORE building anything; one miss = nothing posted.
    let mut codes = Vec::with_capacity(strokes.len());
    for s in strokes {
        let code = match s.key {
            MacKey::Code(k) => k,
            MacKey::Char(c) => keycode_for_char(c, s.mods.cmd).ok_or_else(|| {
                format!(
                    "keyboard layout key '{c}' not resolved yet — nothing injected (open the Companion window once)"
                )
            })?,
        };
        codes.push(code);
    }
    // 2. Build every event for the whole sequence (creation is the only fallible step).
    let src = hid_source()?;
    let mut planned = Vec::new();
    for (s, code) in strokes.iter().zip(codes) {
        plan_stroke(&src, code, s, &mut planned)?;
    }
    // 3. Post.
    post_planned(planned);
    Ok(())
}

pub fn post_key(keycode: u16) -> Result<(), String> {
    require_permission()?;
    let src = hid_source()?;
    let planned = vec![
        Planned { event: key_event(&src, keycode, true, 0, None)?, pause_after: Duration::ZERO },
        Planned { event: key_event(&src, keycode, false, 0, None)?, pause_after: Duration::ZERO },
    ];
    post_planned(planned);
    Ok(())
}

pub fn type_text(text: &str) -> Result<(), String> {
    require_permission()?;
    let chunks = keymap::utf16_chunks(text, keymap::MAC_UNICODE_CHUNK);
    if chunks.is_empty() {
        return Ok(());
    }
    let src = hid_source()?;
    let mut planned = Vec::with_capacity(chunks.len() * 2);
    let last = chunks.len() - 1;
    for (i, chunk) in chunks.iter().enumerate() {
        let down = key_event(&src, 0, true, 0, None)?;
        down.set_string_from_utf16_unchecked(chunk);
        let up = key_event(&src, 0, false, 0, None)?;
        up.set_string_from_utf16_unchecked(chunk);
        planned.push(Planned { event: down, pause_after: Duration::ZERO });
        planned.push(Planned {
            event: up,
            pause_after: if i == last { Duration::ZERO } else { CHUNK_PAUSE },
        });
    }
    post_planned(planned);
    Ok(())
}

/// Two quick down/up taps of a modifier key (FlagsChanged events). `flags`
/// includes the device-specific bit (plan D5).
pub fn double_tap_modifier(keycode: u16, flags: u64) -> Result<(), String> {
    require_permission()?;
    let src = hid_source()?;
    let ft = Some(CGEventType::FlagsChanged);
    let mut planned = Vec::with_capacity(4);
    for tap in 0..2 {
        planned.push(Planned { event: key_event(&src, keycode, true, flags, ft)?, pause_after: Duration::ZERO });
        planned.push(Planned {
            event: key_event(&src, keycode, false, 0, ft)?,
            pause_after: if tap == 0 { DOUBLE_TAP_GAP } else { Duration::ZERO },
        });
    }
    post_planned(planned);
    Ok(())
}

// ───────────────────────────── Accessibility (AX) ─────────────────────────────

/// RAII wrapper over an `AXUIElementRef` (released on drop by `CFType`).
struct AxElement(CFType);

impl AxElement {
    /// Take ownership of a Create/Copy-rule ref.
    fn from_create(p: AXUIElementRef) -> Option<Self> {
        if p.is_null() {
            None
        } else {
            Some(AxElement(unsafe { CFType::wrap_under_create_rule(p) }))
        }
    }

    fn application(pid: i32) -> Option<Self> {
        let el = Self::from_create(unsafe { AXUIElementCreateApplication(pid) })?;
        unsafe { AXUIElementSetMessagingTimeout(el.raw(), AX_MESSAGING_TIMEOUT_SECS) };
        Some(el)
    }

    fn system_wide() -> Option<Self> {
        let el = Self::from_create(unsafe { AXUIElementCreateSystemWide() })?;
        unsafe { AXUIElementSetMessagingTimeout(el.raw(), AX_MESSAGING_TIMEOUT_SECS) };
        Some(el)
    }

    fn raw(&self) -> AXUIElementRef {
        self.0.as_CFTypeRef()
    }

    /// `AXUIElementCopyAttributeValue`. Attribute names are CFSTR macros, not
    /// linkable symbols, so they are built with `from_static_string`.
    fn attr(&self, name: &'static str) -> Option<CFType> {
        let key = CFString::from_static_string(name);
        let mut out: CFTypeRef = std::ptr::null();
        let err = unsafe { AXUIElementCopyAttributeValue(self.raw(), key.as_concrete_TypeRef(), &mut out) };
        if err != AX_SUCCESS || out.is_null() {
            return None;
        }
        Some(unsafe { CFType::wrap_under_create_rule(out) })
    }

    fn string_attr(&self, name: &'static str) -> Option<String> {
        self.attr(name)?.downcast_into::<CFString>().map(|s| s.to_string())
    }

    fn bool_attr(&self, name: &'static str) -> Option<bool> {
        self.attr(name)?.downcast_into::<CFBoolean>().map(bool::from)
    }

    fn element_attr(&self, name: &'static str) -> Option<AxElement> {
        // The value is an AXUIElementRef; keep it as a plain CFType.
        self.attr(name).map(AxElement)
    }

    fn elements_attr(&self, name: &'static str) -> Vec<AxElement> {
        let Some(arr) = self.attr(name).and_then(|v| v.downcast_into::<CFArray>()) else {
            return Vec::new();
        };
        arr.iter()
            .map(|item| {
                let p = *item as CFTypeRef;
                AxElement(unsafe { CFType::wrap_under_get_rule(p) })
            })
            .collect()
    }

    fn set_bool(&self, name: &'static str, value: bool) -> bool {
        let key = CFString::from_static_string(name);
        let v = if value { CFBoolean::true_value() } else { CFBoolean::false_value() };
        unsafe { AXUIElementSetAttributeValue(self.raw(), key.as_concrete_TypeRef(), v.as_CFTypeRef()) == AX_SUCCESS }
    }

    fn perform(&self, action: &'static str) -> bool {
        let a = CFString::from_static_string(action);
        unsafe { AXUIElementPerformAction(self.raw(), a.as_concrete_TypeRef()) == AX_SUCCESS }
    }

    fn pid(&self) -> Option<i32> {
        let mut pid: i32 = 0;
        let err = unsafe { AXUIElementGetPid(self.raw(), &mut pid) };
        if err == AX_SUCCESS {
            Some(pid)
        } else {
            None
        }
    }
}

/// PID of the frontmost application (system-wide `AXFocusedApplication`).
pub fn frontmost_pid() -> Option<i32> {
    let sys = AxElement::system_wide()?;
    sys.element_attr("AXFocusedApplication")?.pid()
}

fn cf_number_i32(dict: &CFDictionary<*const c_void, *const c_void>, key: CFStringRef) -> Option<i32> {
    let v = dict.find(key as *const c_void)?;
    unsafe { CFNumber::wrap_under_get_rule(*v as CFNumberRef) }.to_i32()
}

/// Layer-0 owner PIDs of a `CGWindowListCopyWindowInfo` pass, in list order,
/// deduplicated.
fn window_list_pids(option: u32) -> Vec<i32> {
    let mut out: Vec<i32> = Vec::new();
    let Some(list) = copy_window_info(option, kCGNullWindowID) else {
        return out;
    };
    // The array holds CFDictionary refs (Get rule).
    let raw = list.get_all_values();
    for p in raw {
        let dict: CFDictionary<*const c_void, *const c_void> =
            unsafe { CFDictionary::wrap_under_get_rule(p as CFDictionaryRef) };
        let layer = unsafe { cf_number_i32(&dict, kCGWindowLayer) };
        if layer != Some(0) {
            continue;
        }
        if let Some(pid) = unsafe { cf_number_i32(&dict, kCGWindowOwnerPID) } {
            if !out.contains(&pid) {
                out.push(pid);
            }
        }
    }
    out
}

/// Candidate PIDs: on-screen windows front-to-back, then hidden / minimised
/// apps from the all-windows pass; our own PID dropped.
fn candidate_pids(exclude_pid: i32) -> Vec<i32> {
    let mut pids = window_list_pids(kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements);
    for pid in window_list_pids(kCGWindowListOptionAll) {
        if !pids.contains(&pid) {
            pids.push(pid);
        }
    }
    pids.retain(|p| *p != exclude_pid);
    pids
}

/// First window of `app` whose `AXTitle` contains `needle_lower` (case-insensitive).
fn matching_window(app: &AxElement, needle_lower: &str) -> Option<AxElement> {
    app.elements_attr("AXWindows").into_iter().find(|w| {
        w.string_attr("AXTitle")
            .map(|t| t.to_lowercase().contains(needle_lower))
            .unwrap_or(false)
    })
}

/// Resolve `(pid, app element, window element)` for the first window whose
/// title contains `needle`. Fast path through `hint_pid`, then a full scan.
fn find_window(needle: &str, exclude_pid: i32, hint_pid: Option<i32>) -> Option<(i32, AxElement, AxElement)> {
    let needle_lower = needle.trim().to_lowercase();
    if needle_lower.is_empty() {
        return None;
    }
    if let Some(pid) = hint_pid.filter(|p| *p != exclude_pid) {
        if let Some(app) = AxElement::application(pid) {
            if let Some(win) = matching_window(&app, &needle_lower) {
                return Some((pid, app, win));
            }
        }
    }
    for pid in candidate_pids(exclude_pid) {
        if Some(pid) == hint_pid {
            continue; // already tried
        }
        let Some(app) = AxElement::application(pid) else { continue };
        if let Some(win) = matching_window(&app, &needle_lower) {
            return Some((pid, app, win));
        }
    }
    None
}

pub fn find_window_pid_by_title(
    needle: &str,
    exclude_pid: i32,
    hint_pid: Option<i32>,
) -> Option<i32> {
    find_window(needle, exclude_pid, hint_pid).map(|(pid, _, _)| pid)
}

pub fn focus_window_by_title(
    needle: &str,
    exclude_pid: i32,
    hint_pid: Option<i32>,
) -> Result<i32, String> {
    if !input_permission_granted() {
        return Err("Accessibility permission not granted — cannot focus window".to_string());
    }
    let (pid, app, win) = find_window(needle, exclude_pid, hint_pid)
        .ok_or_else(|| format!("{ERR_NO_MATCH} {needle:?}"))?;

    app.set_bool("AXHidden", false);
    win.set_bool("AXMinimized", false);
    app.set_bool("AXFrontmost", true);
    win.perform("AXRaise");
    win.set_bool("AXMain", true);

    let deadline = Instant::now() + FRONTMOST_POLL;
    let mut front = frontmost_pid() == Some(pid);
    while !front && Instant::now() < deadline {
        sleep(FRONTMOST_POLL_STEP);
        front = frontmost_pid() == Some(pid);
    }
    if !front {
        // Fail closed: injecting while another app is frontmost would type into it.
        return Err(format!(
            "found window {needle:?} (pid {pid}) but macOS did not bring it to the front — nothing injected"
        ));
    }
    sleep(FOCUS_SETTLE);
    Ok(pid)
}

/// Press a menu-bar item of `pid` (never the frontmost app, never ourselves).
/// Walks the menu bar's items and their `AXMenu` children one level deep; picks
/// the first enabled item that `matcher` classifies as `prefer`, else the first
/// enabled match of any kind. Returns the kind that was pressed.
pub fn press_menu_item(
    pid: i32,
    matcher: fn(&str) -> Option<DictationMenuItem>,
    prefer: DictationMenuItem,
) -> Result<DictationMenuItem, String> {
    if pid == std::process::id() as i32 {
        return Err("refusing to press a menu item in our own process".to_string());
    }
    require_permission()?;
    let app = AxElement::application(pid).ok_or_else(|| format!("cannot access pid {pid}"))?;
    let bar = app
        .element_attr("AXMenuBar")
        .ok_or_else(|| format!("pid {pid} has no accessible menu bar"))?;

    let mut first_any: Option<(AxElement, DictationMenuItem)> = None;
    for top in bar.elements_attr("AXChildren") {
        for menu in top.elements_attr("AXChildren") {
            for item in menu.elements_attr("AXChildren") {
                let Some(title) = item.string_attr("AXTitle") else { continue };
                let Some(kind) = matcher(&title) else { continue };
                if item.bool_attr("AXEnabled") == Some(false) {
                    continue;
                }
                if kind == prefer {
                    return if item.perform("AXPress") {
                        Ok(kind)
                    } else {
                        Err(format!("AXPress on menu item {title:?} failed"))
                    };
                }
                if first_any.is_none() {
                    first_any = Some((item, kind));
                }
            }
        }
    }
    match first_any {
        Some((item, kind)) => {
            if item.perform("AXPress") {
                Ok(kind)
            } else {
                Err("AXPress on the dictation menu item failed".to_string())
            }
        }
        None => Err("no matching dictation menu item found in the target app's menu bar".to_string()),
    }
}
