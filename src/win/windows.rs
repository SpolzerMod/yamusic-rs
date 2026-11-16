//! Тонкие обёртки над Win32: показать/спрятать окно, поверх всех, фокус, мониторы.

use std::ffi::c_void;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND};
use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentProcess, GetCurrentThreadId, SetProcessWorkingSetSize};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows_sys::Win32::System::Registry::{
    RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

fn h(x: isize) -> HWND {
    x as HWND
}

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn is_visible(hwnd: isize) -> bool {
    unsafe { IsWindowVisible(h(hwnd)) != 0 }
}

pub fn is_iconic(hwnd: isize) -> bool {
    unsafe { IsIconic(h(hwnd)) != 0 }
}

pub fn show(hwnd: isize) {
    unsafe {
        if IsIconic(h(hwnd)) != 0 {
            ShowWindow(h(hwnd), SW_RESTORE);
        } else {
            ShowWindow(h(hwnd), SW_SHOW);
        }
    }
}

pub fn hide(hwnd: isize) {
    unsafe {
        ShowWindow(h(hwnd), SW_HIDE);
    }
}

/// SetForegroundWindow с обходом ограничений Windows на кражу фокуса.
pub fn force_foreground(hwnd: isize) {
    unsafe {
        let target = h(hwnd);
        if SetForegroundWindow(target) != 0 && GetForegroundWindow() == target {
            return;
        }
        let fg = GetForegroundWindow();
        let fg_thread = GetWindowThreadProcessId(fg, null_mut());
        let me = GetCurrentThreadId();
        if fg_thread != 0 && fg_thread != me {
            AttachThreadInput(me, fg_thread, 1);
            BringWindowToTop(target);
            SetForegroundWindow(target);
            AttachThreadInput(me, fg_thread, 0);
        } else {
            BringWindowToTop(target);
        }
    }
}

pub fn dpi_scale(hwnd: isize) -> f32 {
    let dpi = unsafe { GetDpiForWindow(h(hwnd)) };
    if dpi == 0 {
        1.0
    } else {
        dpi as f32 / 96.0
    }
}

/// Значок из ресурсов нашего exe (кладёт build.rs), иначе стандартный.
pub fn app_icon(size: i32) -> HICON {
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    unsafe {
        let icon = LoadImageW(GetModuleHandleW(null()), 1 as _, IMAGE_ICON, size, size, LR_DEFAULTCOLOR);
        if !icon.is_null() {
            return icon as HICON;
        }
        LoadIconW(null_mut(), IDI_APPLICATION)
    }
}

/// Значок окна в заголовке, на панели задач и в Alt+Tab.
pub fn set_icon(hwnd: isize) {
    unsafe {
        let big = app_icon(GetSystemMetrics(SM_CXICON));
        let small = app_icon(GetSystemMetrics(SM_CXSMICON));
        SendMessageW(h(hwnd), WM_SETICON, ICON_BIG as usize, big as isize);
        SendMessageW(h(hwnd), WM_SETICON, ICON_SMALL as usize, small as isize);
    }
}

/// Скруглённые углы окна на Windows 11 (на 10 просто ничего не делает).
pub fn round_corners(hwnd: isize) {
    let pref = DWMWCP_ROUND;
    unsafe {
        DwmSetWindowAttribute(
            h(hwnd),
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            &pref as *const _ as *const c_void,
            std::mem::size_of_val(&pref) as u32,
        );
    }
}

/// Системный диалог выбора картинки (JPEG, PNG). `None` — пользователь отказался.
pub fn pick_image(owner: isize, title: &str) -> Option<String> {
    use windows_sys::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_PATHMUSTEXIST, OPENFILENAMEW};
    let filter: Vec<u16> = "Изображения (*.jpg, *.png)\0*.jpg;*.jpeg;*.png\0\0".encode_utf16().collect();
    let title = wide(title);
    let mut buf = vec![0u16; 1024];
    unsafe {
        let mut ofn: OPENFILENAMEW = std::mem::zeroed();
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = h(owner);
        ofn.lpstrFilter = filter.as_ptr();
        ofn.lpstrFile = buf.as_mut_ptr();
        ofn.nMaxFile = buf.len() as u32;
        ofn.lpstrTitle = title.as_ptr();
        ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST;
        if GetOpenFileNameW(&mut ofn) == 0 {
            return None;
        }
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

pub fn open_url(url: &str) {
    let op = wide("open");
    let u = wide(url);
    unsafe {
        ShellExecuteW(null_mut(), op.as_ptr(), u.as_ptr(), null(), null(), SW_SHOWNORMAL);
    }
}

/// Отдать системе память, которую процесс сейчас не трогает (в основном —
/// буферы драйвера видеокарты). Вызываем, когда окно спрятано в трей.
pub fn trim_memory() {
    unsafe {
        SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

/// Отдать системе свободные блоки кучи (после выброса обложек) и ужать рабочий набор.
pub fn release_memory() {
    use windows_sys::Win32::System::Memory::{GetProcessHeap, HeapCompact};
    unsafe {
        HeapCompact(GetProcessHeap(), 0);
    }
    trim_memory();
}

/// Светлая ли тема Windows для приложений.
pub fn system_light_theme() -> bool {
    let path = wide(r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize");
    let name = wide("AppsUseLightTheme");
    let mut value: u32 = 1;
    let mut size = 4u32;
    unsafe {
        RegGetValueW(HKEY_CURRENT_USER, path.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, null_mut(), &mut value as *mut u32 as *mut c_void, &mut size);
    }
    value != 0
}

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_NAME: &str = "yamusic-rs";

/// Команда автозапуска: этот exe, сразу в трее.
fn autostart_command() -> Option<String> {
    Some(format!("\"{}\" --tray", std::env::current_exe().ok()?.display()))
}

/// Записан ли автозапуск вместе с Windows.
pub fn autostart_enabled() -> bool {
    let (key, name) = (wide(RUN_KEY), wide(RUN_NAME));
    unsafe { RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, null_mut(), null_mut(), null_mut()) == 0 }
}

/// Включить или выключить автозапуск (HKCU\…\Run, права администратора не нужны).
pub fn set_autostart(on: bool) -> bool {
    let (key, name) = (wide(RUN_KEY), wide(RUN_NAME));
    unsafe {
        if !on {
            let r = RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr());
            return r == 0 || r == ERROR_FILE_NOT_FOUND;
        }
        let Some(cmd) = autostart_command() else { return false };
        let data = wide(&cmd);
        RegSetKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr(), REG_SZ, data.as_ptr().cast(), (data.len() * 2) as u32) == 0
    }
}

/// Exe перенесли в другую папку — поправить путь в автозапуске, если он включён.
pub fn refresh_autostart() {
    if autostart_enabled() {
        set_autostart(true);
    }
}

pub fn is_window(hwnd: isize) -> bool {
    unsafe { IsWindow(h(hwnd)) != 0 }
}

/// Системный диалог выбора папки, открывается на `start`. `None` — пользователь отказался.
pub fn pick_folder(owner: isize, title: &str, start: &str) -> Option<String> {
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::UI::Shell::{
        SHBrowseForFolderW, SHGetPathFromIDListW, BFFM_INITIALIZED, BFFM_SETSELECTIONW, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW,
    };
    unsafe extern "system" fn on_init(hwnd: HWND, msg: u32, _: isize, data: isize) -> i32 {
        if msg == BFFM_INITIALIZED {
            SendMessageW(hwnd, BFFM_SETSELECTIONW, 1, data);
        }
        0
    }
    let title = wide(title);
    let start = wide(start);
    let mut buf = vec![0u16; 1024];
    unsafe {
        let mut bi: BROWSEINFOW = std::mem::zeroed();
        bi.hwndOwner = h(owner);
        bi.lpszTitle = title.as_ptr();
        bi.ulFlags = BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE;
        bi.lpfn = Some(on_init);
        bi.lParam = start.as_ptr() as isize;
        let pidl = SHBrowseForFolderW(&bi);
        if pidl.is_null() {
            return None;
        }
        let ok = SHGetPathFromIDListW(pidl, buf.as_mut_ptr());
        CoTaskMemFree(pidl as *const c_void);
        if ok == 0 {
            return None;
        }
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..n]))
}

/// Рабочая область монитора с окном (без панели задач), в физических пикселях: лево, верх, право, низ.
pub fn work_area(hwnd: isize) -> (i32, i32, i32, i32) {
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    unsafe {
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        GetMonitorInfoW(MonitorFromWindow(h(hwnd), MONITOR_DEFAULTTONEAREST), &mut mi);
        let r = mi.rcWork;
        (r.left, r.top, r.right, r.bottom)
    }
}
