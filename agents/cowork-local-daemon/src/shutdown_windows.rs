use std::{future::pending, ptr, thread};

use tokio::sync::oneshot;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, GetWindowLongPtrW,
        RegisterClassW, SetWindowLongPtrW, TranslateMessage, CREATESTRUCTW, GWLP_USERDATA, MSG,
        WM_NCCREATE, WM_NCDESTROY, WM_QUERYENDSESSION, WNDCLASSW,
    },
};

struct WindowState {
    sender: Option<oneshot::Sender<()>>,
}

pub async fn session_end_signal() {
    let (sender, receiver) = oneshot::channel();
    thread::Builder::new()
        .name("cowork-session-end".to_owned())
        .spawn(move || message_window(sender))
        .ok();
    if receiver.await.is_err() {
        // Console signals remain active in the caller. A failed window setup
        // must not be interpreted as a shutdown request.
        pending::<()>().await;
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn message_window(sender: oneshot::Sender<()>) {
    let class_name = wide("OpenCoworkLocalDaemonShutdownWindow");
    let state = Box::into_raw(Box::new(WindowState {
        sender: Some(sender),
    }));
    unsafe {
        let instance = GetModuleHandleW(ptr::null());
        let mut window_class: WNDCLASSW = std::mem::zeroed();
        window_class.lpfnWndProc = Some(window_proc);
        window_class.hInstance = instance;
        window_class.lpszClassName = class_name.as_ptr();
        if RegisterClassW(&window_class) == 0 {
            drop(Box::from_raw(state));
            return;
        }
        // This is deliberately a hidden top-level window, not HWND_MESSAGE:
        // Windows broadcasts WM_QUERYENDSESSION only to top-level windows.
        let window = CreateWindowExW(
            0,
            class_name.as_ptr(),
            class_name.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            state.cast(),
        );
        if window.is_null() {
            // CreateWindow may already have emitted WM_NCDESTROY. Leaking this
            // process-lifetime sentinel is safer than a possible double free.
            return;
        }
        let mut message: MSG = std::mem::zeroed();
        while GetMessageW(&mut message, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
}

unsafe extern "system" fn window_proc(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = lparam as *const CREATESTRUCTW;
        if !create.is_null() {
            unsafe {
                SetWindowLongPtrW(window, GWLP_USERDATA, (*create).lpCreateParams as isize);
            }
        }
        return 1;
    }
    let state = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) as *mut WindowState };
    if message == WM_QUERYENDSESSION {
        if !state.is_null() {
            if let Some(sender) = unsafe { &mut *state }.sender.take() {
                let _ = sender.send(());
            }
        }
        return 1;
    }
    if message == WM_NCDESTROY && !state.is_null() {
        unsafe {
            SetWindowLongPtrW(window, GWLP_USERDATA, 0);
            drop(Box::from_raw(state));
        }
        return 0;
    }
    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}
