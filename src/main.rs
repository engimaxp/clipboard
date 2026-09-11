//! 剪贴板历史 —— 自动监听剪贴板，新内容换行追加到历史记录。
//!
//! - Windows 下用剪贴板序列号实时检测变化（50ms），其他平台 500ms 轮询兜底
//! - 确认是新的复制动作（包括重新复制相同内容）就追加到历史
//! - 历史持久化到系统应用数据目录下的 history.json
//! - 关闭窗口后驻留系统托盘继续监听，托盘右键可退出
//! - 点击历史记录即可重新复制到剪贴板；自己写入的内容不会被重复记录

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use arboard::Clipboard;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, State, WindowEvent};

/// 序列号轮询间隔：Windows 下以此频率检查剪贴板是否变化（一次轻量 Win32 调用）
const SEQ_POLL: Duration = Duration::from_millis(50);
/// 兜底轮询：序列号/通知机制漏检时的慢速全量读取
#[cfg(target_os = "windows")]
const FALLBACK_POLL: Duration = Duration::from_secs(2);
#[cfg(not(target_os = "windows"))]
const FALLBACK_POLL: Duration = Duration::from_millis(500);
/// 历史记录最大条数（超出后丢弃最旧记录）
const MAX_ENTRIES: usize = 1000;

#[derive(Clone, Serialize, Deserialize)]
struct HistoryEntry {
    text: String,
    #[serde(default)]
    timestamp: i64,
}

struct Inner {
    entries: Mutex<Vec<HistoryEntry>>,
    last_seen: Mutex<String>,
    /// 我们自己写入剪贴板时置位，用于在变化事件中区分「自己的写入」与「用户重新复制」
    self_write: AtomicBool,
    listening: AtomicBool,
    /// 非文本内容提示去抖：(上次提示的原因, 上次提示时间)
    last_skip: Mutex<(String, Instant)>,
    history_path: PathBuf,
}

#[derive(Clone)]
struct AppState(Arc<Inner>);

fn now_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn read_clipboard_text() -> Option<String> {
    Clipboard::new().ok()?.get_text().ok()
}

fn save(path: &PathBuf, entries: &[HistoryEntry]) {
    let json = serde_json::to_string_pretty(entries).unwrap_or_else(|_| "[]".into());
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    // 先写临时文件再改名，避免写一半损坏历史文件
    let tmp = path.with_extension("json.tmp");
    if let Ok(mut f) = fs::File::create(&tmp) {
        let _ = f.write_all(json.as_bytes());
        drop(f);
        let _ = fs::rename(&tmp, path);
    }
}

fn load(path: &PathBuf) -> Vec<HistoryEntry> {
    fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Windows 剪贴板原生 API：序列号每次内容变化（含清空、重写相同内容）都会递增，
/// 用它做超快变化检测；IsClipboardFormatAvailable 用于识别非文本内容类型。
#[cfg(target_os = "windows")]
mod win_clipboard {
    #[link(name = "user32")]
    extern "system" {
        fn GetClipboardSequenceNumber() -> u32;
        fn IsClipboardFormatAvailable(format: u32) -> i32;
    }

    const CF_BITMAP: u32 = 2;
    const CF_DIB: u32 = 8;
    const CF_HDROP: u32 = 15;

    pub fn seq() -> u32 {
        unsafe { GetClipboardSequenceNumber() }
    }

    pub fn skip_reason() -> &'static str {
        unsafe {
            if IsClipboardFormatAvailable(CF_DIB) != 0 || IsClipboardFormatAvailable(CF_BITMAP) != 0 {
                return "image";
            }
            if IsClipboardFormatAvailable(CF_HDROP) != 0 {
                return "files";
            }
        }
        "other"
    }
}

#[cfg(not(target_os = "windows"))]
mod win_clipboard {
    pub fn seq() -> u32 {
        0
    }
    pub fn skip_reason() -> &'static str {
        "other"
    }
}

/// 后台监听线程：
/// - Windows：50ms 检查剪贴板序列号，变化时立即读取处理；另有兜底轮询防漏检
/// - 其他平台：纯轮询
fn spawn_watcher(app: AppHandle, state: AppState) {
    std::thread::spawn(move || {
        let mut last_seq = win_clipboard::seq();
        let mut last_check = Instant::now();
        loop {
            if state.0.listening.load(Ordering::Relaxed) {
                let seq = win_clipboard::seq();
                if seq != last_seq {
                    // 序列号变化 = 剪贴板确实被改动（包括重新复制相同内容）
                    last_seq = seq;
                    last_check = Instant::now();
                    handle_clipboard_event(&app, &state, true);
                } else if last_check.elapsed() >= FALLBACK_POLL {
                    // 兜底：某些场景序列号可能不变化，慢速全量读取补偿
                    last_check = Instant::now();
                    handle_clipboard_event(&app, &state, false);
                }
            } else {
                // 暂停监听：同步 last_seen 防止恢复后误记录旧内容，并重置序列号基线
                if last_check.elapsed() >= FALLBACK_POLL {
                    last_check = Instant::now();
                    if let Some(t) = read_clipboard_text() {
                        *state.0.last_seen.lock().unwrap() = t;
                    }
                }
                last_seq = win_clipboard::seq();
            }
            std::thread::sleep(SEQ_POLL);
        }
    });
}

/// 处理一次剪贴板事件。is_notification=true 表示确认剪贴板发生了变化
/// （序列号变更或系统通知），false 表示只是兜底轮询。
fn handle_clipboard_event(app: &AppHandle, state: &AppState, is_notification: bool) {
    match read_clipboard_text_retry() {
        Ok(Some(text)) => {
            if text.trim().is_empty() {
                *state.0.last_seen.lock().unwrap() = String::new();
                return;
            }
            let mut last_seen = state.0.last_seen.lock().unwrap();
            let self_write = state.0.self_write.swap(false, Ordering::Relaxed);
            if *last_seen == text {
                // 内容与上次一致：
                // - 自己的写入 → 跳过
                // - 兜底轮询 → 视为无变化，跳过
                // - 确认变化事件 → 用户重新复制了相同内容 → 记为新记录
                if self_write || !is_notification {
                    return;
                }
            }
            record_entry(app, state, &text);
            *last_seen = text;
        }
        Ok(None) => {
            // 剪贴板里没有文本：图片 / 文件 / 已清空
            if is_notification {
                *state.0.last_seen.lock().unwrap() = String::new();
                let reason = win_clipboard::skip_reason();
                emit_skip_debounced(app, state, reason);
            }
        }
        Err(err) => {
            // 读取失败（如写入方仍占用剪贴板）：记录日志，交给兜底轮询重试
            log_line(state, &format!("clipboard read failed: {err}"));
        }
    }
}

/// 读取剪贴板文本；短暂重试以规避「写入方刚写入时仍占用剪贴板」的窗口期
fn read_clipboard_text_retry() -> Result<Option<String>, String> {
    for attempt in 0..4 {
        match Clipboard::new().and_then(|mut c| c.get_text()) {
            Ok(text) => return Ok(Some(text)),
            Err(arboard::Error::ContentNotAvailable) => return Ok(None),
            Err(e) => {
                if attempt < 3 {
                    std::thread::sleep(Duration::from_millis(30));
                    continue;
                }
                return Err(e.to_string());
            }
        }
    }
    Ok(None)
}

/// 追加一条历史记录并通知前端（不去重：「确认变化 + 相同内容」也是新的复制动作）
fn record_entry(app: &AppHandle, state: &AppState, text: &str) {
    {
        let mut entries = state.0.entries.lock().unwrap();
        entries.push(HistoryEntry {
            text: text.to_string(),
            timestamp: now_ts(),
        });
        if entries.len() > MAX_ENTRIES {
            let excess = entries.len() - MAX_ENTRIES;
            entries.drain(0..excess);
        }
        save(&state.0.history_path, &entries);
    }
    let all = state.0.entries.lock().unwrap().clone();
    let _ = app.emit("history-updated", all);
}

/// 提示「剪贴板内容不是文本」（3 秒去抖，避免反复弹提示）
fn emit_skip_debounced(app: &AppHandle, state: &AppState, reason: &'static str) {
    let mut last = state.0.last_skip.lock().unwrap();
    let now = Instant::now();
    let recently_same = last.0 == reason && now.duration_since(last.1) < Duration::from_secs(3);
    last.0 = reason.to_string();
    last.1 = now;
    drop(last);
    if recently_same {
        return;
    }
    let _ = app.emit("clipboard-skipped", reason);
    log_line(state, &format!("skipped non-text clipboard: {reason}"));
}

// ---------- 前端命令 ----------

#[tauri::command]
fn get_history(state: State<AppState>) -> Vec<HistoryEntry> {
    state.0.entries.lock().unwrap().clone()
}

#[tauri::command]
fn get_listening(state: State<AppState>) -> bool {
    state.0.listening.load(Ordering::Relaxed)
}

#[tauri::command]
fn set_listening(app: AppHandle, state: State<AppState>, listening: bool) {
    state.0.listening.store(listening, Ordering::Relaxed);
    let _ = app.emit("listening-changed", listening);
}

#[tauri::command]
fn clear_history(app: AppHandle, state: State<AppState>) {
    {
        let mut entries = state.0.entries.lock().unwrap();
        entries.clear();
        save(&state.0.history_path, &entries);
    }
    let _ = app.emit("history-updated", Vec::<HistoryEntry>::new());
}

/// 把某条历史记录重新复制到剪贴板（标记为自己写入，避免被重复记录进历史）
#[tauri::command]
fn copy_text(state: State<AppState>, text: String) -> Result<(), String> {
    write_clipboard_owned(&state, text)
}

/// 写入剪贴板并标记为「自己写入」：随后的事件会被跳过，不会重复记入历史。
/// 先更新 last_seen 再写剪贴板，避免监听线程在两者之间读到旧值产生竞态。
fn write_clipboard_owned(state: &AppState, text: String) -> Result<(), String> {
    state.0.self_write.store(true, Ordering::Relaxed);
    let prev = {
        let mut ls = state.0.last_seen.lock().unwrap();
        let prev = ls.clone();
        *ls = text.clone();
        prev
    };
    let mut clip = Clipboard::new().map_err(|e| e.to_string())?;
    if let Err(e) = clip.set_text(text) {
        // 写入失败：回滚 last_seen 并清除标记
        *state.0.last_seen.lock().unwrap() = prev;
        state.0.self_write.store(false, Ordering::Relaxed);
        return Err(e.to_string());
    }
    Ok(())
}

/// 把全部历史记录按时间顺序、每条换行合并成一段文本复制到剪贴板
#[tauri::command]
fn copy_all(state: State<AppState>) -> Result<usize, String> {
    let entries = state.0.entries.lock().unwrap();
    let joined = join_entries(&entries);
    let n = entries.len();
    drop(entries);

    if joined.trim().is_empty() {
        return Err("历史为空，没有可复制的内容".into());
    }

    write_clipboard_owned(&state, joined)?;
    Ok(n)
}

/// 把条目按时间顺序用换行连接成一段文本（每条记录一行）
fn join_entries(entries: &[HistoryEntry]) -> String {
    entries
        .iter()
        .map(|e| e.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// 前端自诊断：把 UI 侧日志追加到应用数据目录的 ui.log（排查监听/渲染问题用）
#[tauri::command]
fn ui_report(state: State<AppState>, msg: String) {
    log_line(&state, &msg);
}

/// 追加一行日志到 ui.log
fn log_line(state: &AppState, msg: &str) {
    let log_path = state.0.history_path.with_file_name("ui.log");
    if let Some(parent) = log_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&log_path) {
        let line = format!("[{}] {}\n", now_ts(), msg);
        let _ = f.write_all(line.as_bytes());
    }
}

// ---------- 系统托盘 ----------

fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    let toggle = MenuItem::with_id(app, "toggle", "显示 / 隐藏", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&toggle, &quit])?;

    TrayIconBuilder::with_id("main-tray")
        .icon(app.default_window_icon().expect("缺少默认图标").clone())
        .tooltip("剪贴板历史")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "toggle" => toggle_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_window(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

fn toggle_window(app: &AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        if win.is_visible().unwrap_or(false) {
            let _ = win.hide();
        } else {
            let _ = win.show();
            let _ = win.unminimize();
            let _ = win.set_focus();
        }
    }
}

// ---------- 入口 ----------

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            // 重复启动时把已有实例的窗口唤出
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.unminimize();
                let _ = win.set_focus();
            }
        }))
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| PathBuf::from("."));
            let history_path = data_dir.join("history.json");
            let mut entries = load(&history_path);
            let state = AppState(Arc::new(Inner {
                entries: Mutex::new(entries.clone()),
                last_seen: Mutex::new(String::new()),
                self_write: AtomicBool::new(false),
                listening: AtomicBool::new(true),
                last_skip: Mutex::new((String::new(), Instant::now())),
                history_path,
            }));

            // 启动时把剪贴板里已有的内容记为第一条（若与最后一条历史不同）
            if let Some(text) = read_clipboard_text() {
                if !text.trim().is_empty() {
                    let mut last_seen = state.0.last_seen.lock().unwrap();
                    *last_seen = text.clone();
                    let is_dup = entries.last().map(|e| e.text == text).unwrap_or(false);
                    if !is_dup {
                        entries.push(HistoryEntry {
                            text,
                            timestamp: now_ts(),
                        });
                        save(&state.0.history_path, &entries);
                        *state.0.entries.lock().unwrap() = entries;
                    }
                }
            }

            setup_tray(app)?;
            spawn_watcher(app.handle().clone(), state.clone());
            app.manage(state);
            Ok(())
        })
        .on_window_event(|window, event| {
            // 点关闭按钮 = 隐藏到托盘，后台继续监听；退出请用托盘菜单
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_history,
            get_listening,
            set_listening,
            clear_history,
            copy_text,
            copy_all,
            ui_report
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(text: &str) -> HistoryEntry {
        HistoryEntry { text: text.into(), timestamp: 0 }
    }

    #[test]
    fn join_entries_single_line() {
        let entries = vec![entry("a"), entry("b"), entry("c")];
        assert_eq!(join_entries(&entries), "a\nb\nc");
    }

    #[test]
    fn join_entries_multiline_keeps_inner_newlines() {
        let entries = vec![entry("第一行\n第二行"), entry("c")];
        assert_eq!(join_entries(&entries), "第一行\n第二行\nc");
    }

    #[test]
    fn join_entries_empty() {
        assert_eq!(join_entries(&[]), "");
    }

    #[test]
    fn join_entries_preserves_order() {
        let entries = vec![entry("old"), entry("new")];
        assert_eq!(join_entries(&entries), "old\nnew");
    }
}
