//! Whether one adapter's `EnumOutputs` list reaches the window's monitor.
//!
//! `DXGI_ERROR_NOT_FOUND` ends the list. Any other error does not, and is
//! not "this adapter does not drive the monitor".

/// `DXGI_ERROR_NOT_FOUND` (`0x887A0002`). `EnumOutputs` returns this when
/// the index is past the last output.
pub const ENUM_OUTPUTS_NOT_FOUND: i32 = 0x887A0002_u32 as i32;

/// Whether `code` is the end of an `EnumOutputs` list.
///
/// True only for [`ENUM_OUTPUTS_NOT_FOUND`]. Any other code is a failure.
pub fn enum_outputs_ended(code: i32) -> bool {
    code == ENUM_OUTPUTS_NOT_FOUND
}

/// What a whole `EnumOutputs` list says about the window's monitor.
#[derive(Debug, PartialEq, Eq)]
pub enum OutputList<E> {
    /// An output is the window's monitor.
    Drives,
    /// The list ended on [`ENUM_OUTPUTS_NOT_FOUND`] with no such output.
    Ended,
    /// A step failed with an error that does not end the list.
    Failed(E),
    /// The steps stopped with no [`ENUM_OUTPUTS_NOT_FOUND`]. Not a clean end.
    Unended,
}

/// Walk `steps` in index order.
///
/// Each item is one `EnumOutputs` result. `Ok(true)` means that output is
/// the window's monitor. `Ok(false)` means it is not, including an output
/// whose description could not be read. `Err` is the step's failure.
/// `is_end` is true only for [`ENUM_OUTPUTS_NOT_FOUND`].
///
/// [`OutputList::Ended`] is the only clean end. [`OutputList::Failed`] and
/// [`OutputList::Unended`] are not.
pub fn output_list<E, I, F>(steps: I, mut is_end: F) -> OutputList<E>
where
    I: IntoIterator<Item = Result<bool, E>>,
    F: FnMut(&E) -> bool,
{
    for step in steps {
        match step {
            Ok(true) => return OutputList::Drives,
            Ok(false) => {}
            Err(error) if is_end(&error) => return OutputList::Ended,
            Err(error) => return OutputList::Failed(error),
        }
    }
    OutputList::Unended
}

#[cfg(test)]
mod tests {
    use super::ENUM_OUTPUTS_NOT_FOUND;
    use super::OutputList;
    use super::enum_outputs_ended;
    use super::output_list;

    fn ended(code: &i32) -> bool {
        enum_outputs_ended(*code)
    }

    #[test]
    fn only_not_found_ends_the_output_list() {
        assert_eq!(ENUM_OUTPUTS_NOT_FOUND, 0x887A0002_u32 as i32);
        assert!(
            enum_outputs_ended(ENUM_OUTPUTS_NOT_FOUND),
            "DXGI_ERROR_NOT_FOUND does not end the list"
        );
        assert_eq!(
            output_list([Ok(false), Err(ENUM_OUTPUTS_NOT_FOUND)], ended),
            OutputList::Ended
        );
        assert_eq!(
            output_list([Ok(false), Ok(true), Err(1)], ended),
            OutputList::Drives
        );
        assert_eq!(
            output_list(std::iter::empty(), ended),
            OutputList::Unended,
            "a list that stops without NOT_FOUND looks like a clean end"
        );

        for code in [
            0,
            1,
            -1,
            0x887A0001_u32 as i32,
            0x887A0005_u32 as i32,
            0x887A0007_u32 as i32,
            0x80004005_u32 as i32,
            0x8007000E_u32 as i32,
        ] {
            assert!(
                !enum_outputs_ended(code),
                "error {code:#x} still ends the output list"
            );
            assert_eq!(
                output_list([Ok(false), Err(code)], ended),
                OutputList::Failed(code),
                "error {code:#x} looks like the end of the list"
            );
            assert_ne!(
                output_list([Err(code), Ok(true)], ended),
                OutputList::Ended,
                "error {code:#x} looks like the end of the list"
            );
        }

        let present = include_str!("present.rs");
        assert!(
            present.contains("enum_outputs_ended"),
            "every EnumOutputs error still ends the list"
        );
        assert!(
            present.contains("output_list"),
            "drives does not ask output_list where the list ends"
        );
        assert_eq!(
            present.matches("drives(&adapter, monitor)?").count(),
            2,
            "an EnumOutputs error is still treated as the end of the list"
        );
        assert!(
            !present.contains("let Ok(output) = (unsafe { adapter.EnumOutputs(i) }) else"),
            "every EnumOutputs error still ends the list"
        );
    }
}
