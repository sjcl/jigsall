use windows_sys::Win32::{
    Foundation::INVALID_HANDLE_VALUE,
    Storage::FileSystem::{GetFileType, FILE_TYPE_UNKNOWN},
    System::Console::{
        AttachConsole, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE,
        STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    },
};

/// Reuse the launching terminal without creating a console on desktop launch.
/// Run before logging or any other standard-stream access.
pub(super) fn attach_parent_console() {
    // SAFETY: These calls use only predefined stream IDs and borrowed process
    // handles. No handles are closed or transferred; startup is single-threaded.
    unsafe {
        let inherited = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE].map(|id| {
            let handle = GetStdHandle(id);
            let valid = !handle.is_null()
                && handle != INVALID_HANDLE_VALUE
                && GetFileType(handle) != FILE_TYPE_UNKNOWN;
            (id, handle, valid)
        });

        // Failure is expected when Explorer (or another GUI) is the parent.
        if AttachConsole(ATTACH_PARENT_PROCESS) == 0 {
            return;
        }

        // Attaching can replace standard handles. Preserve inherited file/pipe
        // redirection, including separate stdout and stderr destinations.
        for (id, handle, valid) in inherited {
            if valid {
                SetStdHandle(id, handle);
            }
        }
    }
}
