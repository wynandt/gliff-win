//! Whether a `CoInitializeEx` HRESULT left COM usable on this thread.

/// `CoInitializeEx` succeeded and this thread is newly initialised.
pub const S_OK: i32 = 0;

/// `CoInitializeEx` succeeded; this thread was already initialised with
/// the same concurrency model.
pub const S_FALSE: i32 = 1;

/// `CoInitializeEx` failed because this thread already has a different
/// concurrency model, but COM is still usable.
pub const RPC_E_CHANGED_MODE: i32 = 0x8001_0106u32 as i32;

/// True when `hr` is `S_OK`, `S_FALSE`, or `RPC_E_CHANGED_MODE`.
pub fn com_init_usable(hr: i32) -> bool {
    hr == S_OK || hr == S_FALSE || hr == RPC_E_CHANGED_MODE
}

#[cfg(test)]
mod tests {
    use super::RPC_E_CHANGED_MODE;
    use super::S_FALSE;
    use super::S_OK;
    use super::com_init_usable;

    #[test]
    fn com_is_usable_after_ok_false_or_a_changed_mode() {
        assert!(com_init_usable(S_OK));
        assert!(com_init_usable(S_FALSE));
        assert!(com_init_usable(RPC_E_CHANGED_MODE));
        assert_eq!(RPC_E_CHANGED_MODE, -2147417850);
    }

    #[test]
    fn any_other_failure_is_not_usable() {
        const E_OUTOFMEMORY: i32 = 0x8007_000Eu32 as i32;
        const E_UNEXPECTED: i32 = 0x8000_FFFFu32 as i32;
        assert!(!com_init_usable(E_OUTOFMEMORY));
        assert!(!com_init_usable(E_UNEXPECTED));
        assert!(!com_init_usable(-1));
    }
}
