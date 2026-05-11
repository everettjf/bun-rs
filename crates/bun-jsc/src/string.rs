use std::ptr;

use bun_jsc_sys as sys;

/// Owned JSC string. RAII; releases the underlying `JSStringRef` on drop.
pub struct JsString {
    raw: sys::JSStringRef,
}

impl JsString {
    /// Create a JSC string from a Rust `&str`. UTF-16 internally.
    pub fn new(s: &str) -> Self {
        let utf16: Vec<u16> = s.encode_utf16().collect();
        let raw = unsafe { sys::JSStringCreateWithCharacters(utf16.as_ptr(), utf16.len()) };
        assert!(!raw.is_null(), "JSStringCreateWithCharacters returned null");
        Self { raw }
    }

    /// Wrap a string already owned by us (we'll release on drop). Public so
    /// helpers in `bun-runtime` can stringify raw JSC values directly.
    pub fn adopt(raw: sys::JSStringRef) -> Self {
        assert!(!raw.is_null());
        Self { raw }
    }

    pub fn as_raw(&self) -> sys::JSStringRef {
        self.raw
    }

    /// Copy the string out as a Rust `String`.
    pub fn to_string(&self) -> String {
        unsafe {
            let cap = sys::JSStringGetMaximumUTF8CStringSize(self.raw);
            if cap == 0 {
                return String::new();
            }
            let mut buf = vec![0u8; cap];
            let written =
                sys::JSStringGetUTF8CString(self.raw, buf.as_mut_ptr() as *mut i8, cap);
            // `written` includes the trailing NUL, so we strip it.
            let len = written.saturating_sub(1);
            buf.truncate(len);
            String::from_utf8_lossy(&buf).into_owned()
        }
    }
}

impl Drop for JsString {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            unsafe { sys::JSStringRelease(self.raw) }
            self.raw = ptr::null_mut();
        }
    }
}
