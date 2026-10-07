//! 写信窗格里的图片：本地图片读取、粘贴图片落盘、截屏取图。
//!
//! 三条来源最后都变成「本地文件路径 + 展示用 data URL」：
//! - 「插入 → 图片」直接给路径；
//! - 粘贴图片把字节写进数据目录；
//! - 截图先把主窗口藏起来拍下整屏，用户框完再按坐标裁。
//!
//! 图片一律按白名单校验（只认常见光栅图、单张不超过 2MB），全程不联网。

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use xcap::Monitor;

use crate::commands::CommandError;
use crate::state::AppState;

/// 截图叠加窗的窗口标签。
const SCREENSHOT_WINDOW: &str = "screenshot";
/// 截图结果发给主窗口的事件名。
const SCREENSHOT_EVENT: &str = "compose:screenshot-ready";
/// 藏窗口到拍屏之间等多久；太短会把还没消失的界面拍进去。
const HIDE_SETTLE: Duration = Duration::from_millis(220);
/// 图片文件名的自增序号，避免同一毫秒内重名。
static IMAGE_SEQ: AtomicU64 = AtomicU64::new(0);

/// 拍下来还没裁的整屏图（RGBA），等用户在叠加窗里框完就裁。
struct PendingShot {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// 同一时刻只会有一张待裁的整屏图。
static PENDING_SHOT: Mutex<Option<PendingShot>> = Mutex::new(None);

/// 给前端的图片信息。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InlineImageDto {
    /// 本地文件路径；发送时按它读字节。
    pub path: String,
    /// 展示文件名。
    pub filename: String,
    /// 嗅探出的图片类型。
    pub mime_type: String,
    /// 网页里可以直接显示的 data URL。
    pub data_url: String,
    /// 字节数。
    pub bytes: usize,
}

/// 落盘目录：`<数据目录>/compose-images`。
fn image_dir(state: &AppState) -> PathBuf {
    let settings = state.settings_snapshot();
    settings
        .effective_data_dir(state.default_data_dir())
        .join("compose-images")
}

/// 拼一个不重名的文件名。
fn next_filename(prefix: &str, extension: &str) -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0);
    let seq = IMAGE_SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{stamp}-{seq}.{extension}")
}

/// 文件头对应的扩展名；用于给落盘的图片起名。
fn extension_for(mime_type: &str) -> &'static str {
    match mime_type {
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" => "bmp",
        "image/avif" => "avif",
        _ => "png",
    }
}

/// 校验图片字节并组装给前端的结构。
fn to_dto(path: PathBuf, filename: String, bytes: Vec<u8>) -> Result<InlineImageDto, CommandError> {
    if bytes.is_empty() {
        return Err(CommandError::input("这张图片是空的，换一张试试"));
    }
    let sniffed = mail_mime::sniff_image_mime(&bytes).ok_or_else(|| {
        CommandError::input("这不是支持的图片格式（只认 PNG / JPEG / GIF / WebP / BMP / AVIF）")
    })?;
    // 用嗅探出的类型再过一次白名单与大小校验，声明类型不参与判断。
    let mime_type = mail_mime::validate_inline_image(sniffed, &bytes)
        .map_err(|error| CommandError::input(format!("图片不能用：{error}")))?;
    let data_url = format!("data:{mime_type};base64,{}", STANDARD.encode(&bytes));
    Ok(InlineImageDto {
        path: path.to_string_lossy().to_string(),
        filename,
        mime_type: mime_type.to_string(),
        data_url,
        bytes: bytes.len(),
    })
}

/// 读本地图片：校验后回 data URL；发送时读的还是这条路径。
#[tauri::command]
pub async fn read_inline_image(path: String) -> Result<InlineImageDto, CommandError> {
    let trimmed = path.trim().to_string();
    if trimmed.is_empty() {
        return Err(CommandError::input("没有选择图片文件"));
    }
    let source = PathBuf::from(&trimmed);
    let filename = source
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "图片".to_string());
    tauri::async_runtime::spawn_blocking(move || {
        let bytes =
            std::fs::read(&source).map_err(|error| CommandError::input(format!("读取图片失败：{error}")))?;
        to_dto(source, filename, bytes)
    })
    .await
    .map_err(|error| CommandError::new(format!("读取图片的任务失败：{error}")))?
}

/// 存粘贴进来的图片：写进数据目录，返回路径与 data URL。
#[tauri::command]
pub async fn save_inline_image(
    state: tauri::State<'_, AppState>,
    data_base64: String,
) -> Result<InlineImageDto, CommandError> {
    let payload = data_base64
        .split_once("base64,")
        .map(|(_, rest)| rest.to_string())
        .unwrap_or(data_base64);
    let bytes = STANDARD
        .decode(payload.trim())
        .map_err(|_| CommandError::input("粘贴的图片数据不完整，请重试"))?;
    let sniffed = mail_mime::sniff_image_mime(&bytes)
        .ok_or_else(|| CommandError::input("剪贴板里的内容不是支持的图片"))?;
    let dir = image_dir(&state);
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&dir)
            .map_err(|error| CommandError::new(format!("建图片目录失败：{error}")))?;
        let filename = next_filename("粘贴图片", extension_for(sniffed));
        let path = dir.join(&filename);
        std::fs::write(&path, &bytes).map_err(|error| CommandError::new(format!("保存图片失败：{error}")))?;
        to_dto(path, filename, bytes)
    })
    .await
    .map_err(|error| CommandError::new(format!("保存图片的任务失败：{error}")))?
}

/// 打开截图叠加窗：先把主窗口藏起来，拍下光标所在那块屏，再让用户框选。
#[tauri::command]
pub async fn open_screenshot_overlay(app: AppHandle) -> Result<(), CommandError> {
    if app.get_webview_window(SCREENSHOT_WINDOW).is_some() {
        return Ok(());
    }
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| CommandError::new("找不到主窗口，截图开不了"))?;
    let cursor = main
        .cursor_position()
        .map_err(|error| CommandError::new(format!("取光标位置失败：{error}")))?;
    let _ = main.hide();

    let cursor_x = cursor.x as i32;
    let cursor_y = cursor.y as i32;
    // 找显示器与拍屏都在后台线程里做：Monitor 不是 Send，一律不出那个线程。
    let source = tauri::async_runtime::spawn_blocking(move || capture_monitor(cursor_x, cursor_y))
        .await
        .map_err(|error| CommandError::new(format!("截图任务失败：{error}")));
    let source = match source {
        Ok(Ok(source)) => source,
        Ok(Err(error)) => {
            show_main_window(&app);
            return Err(error);
        }
        Err(error) => {
            show_main_window(&app);
            return Err(error);
        }
    };
    let (x, y, logical_width, logical_height) =
        (source.x, source.y, source.logical_width, source.logical_height);
    {
        let mut guard = PENDING_SHOT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *guard = Some(source.shot);
    }

    let window = WebviewWindowBuilder::new(
        &app,
        SCREENSHOT_WINDOW,
        WebviewUrl::App("index.html?window=screenshot".into()),
    )
    .title("截图")
    .decorations(false)
    .resizable(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    .position(x, y)
    .inner_size(logical_width, logical_height)
    .build()
    .map_err(|error| {
        clear_pending_shot();
        show_main_window(&app);
        CommandError::new(format!("打开截图窗失败：{error}"))
    })?;
    let _ = window.set_focus();
    Ok(())
}

/// 拍下来的一块屏：窗口要摆的逻辑坐标与尺寸，加上整屏像素。
struct ScreenshotSource {
    /// 窗口摆放的逻辑坐标。
    x: f64,
    /// 窗口摆放的逻辑坐标。
    y: f64,
    /// 窗口的逻辑宽度。
    logical_width: f64,
    /// 窗口的逻辑高度。
    logical_height: f64,
    shot: PendingShot,
}

/// 找要截的显示器：光标所在那块；认不出来就退回主屏。
fn pick_monitor(cursor_x: i32, cursor_y: i32) -> Result<Monitor, CommandError> {
    if let Ok(found) = Monitor::from_point(cursor_x, cursor_y) {
        return Ok(found);
    }
    let mut list = Monitor::all().map_err(|error| CommandError::new(format!("枚举显示器失败：{error}")))?;
    match list.iter().position(|item| item.is_primary().unwrap_or(false)) {
        Some(index) => Ok(list.swap_remove(index)),
        None => list
            .into_iter()
            .next()
            .ok_or_else(|| CommandError::new("这台机器上没有可用的显示器")),
    }
}

/// 拍下整块屏。这个函数只允许在后台线程里调用。
fn capture_monitor(cursor_x: i32, cursor_y: i32) -> Result<ScreenshotSource, CommandError> {
    let monitor = pick_monitor(cursor_x, cursor_y)?;
    let scale = monitor.scale_factor().unwrap_or(1.0).max(0.1) as f64;
    let width = monitor.width().unwrap_or(0);
    let height = monitor.height().unwrap_or(0);
    let x = monitor.x().unwrap_or(0) as f64 / scale;
    let y = monitor.y().unwrap_or(0) as f64 / scale;
    // 等主窗口真的从屏幕上消失，否则会把自己的界面拍进去。
    std::thread::sleep(HIDE_SETTLE);
    let image = monitor
        .capture_image()
        .map_err(|error| CommandError::new(format!("取屏失败：{error}")))?;
    Ok(ScreenshotSource {
        x,
        y,
        logical_width: width as f64 / scale,
        logical_height: height as f64 / scale,
        shot: PendingShot {
            width: image.width(),
            height: image.height(),
            rgba: image.into_raw(),
        },
    })
}

/// 取待裁整屏图的预览（缩成 JPEG，只当背景看，不参与最终清晰度）。
#[tauri::command]
pub async fn take_screenshot_preview() -> Result<String, CommandError> {
    let (width, height, rgba) = {
        let guard = PENDING_SHOT
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let shot = guard
            .as_ref()
            .ok_or_else(|| CommandError::new("截图状态丢了，请重新点一次截图"))?;
        (shot.width, shot.height, shot.rgba.clone())
    };
    tauri::async_runtime::spawn_blocking(move || {
        let buffer = rgba_to_image(width, height, rgba)?;
        let mut encoded = Vec::new();
        xcap::image::DynamicImage::ImageRgba8(buffer)
            .write_to(&mut Cursor::new(&mut encoded), xcap::image::ImageFormat::Jpeg)
            .map_err(|error| CommandError::new(format!("生成截图预览失败：{error}")))?;
        Ok(format!("data:image/jpeg;base64,{}", STANDARD.encode(&encoded)))
    })
    .await
    .map_err(|error| CommandError::new(format!("截图预览任务失败：{error}")))?
}

/// 按用户框的坐标裁下待裁图，存盘后把结果发给主窗口。
///
/// 坐标是物理像素，相对那块屏的左上角。
#[tauri::command]
pub async fn finish_screenshot(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<(), CommandError> {
    let Some(shot) = PENDING_SHOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
    else {
        let _ = close_overlay_and_restore(&app);
        return Err(CommandError::new("截图状态丢了，请重新点一次截图"));
    };
    if width == 0 || height == 0 {
        let _ = close_overlay_and_restore(&app);
        return Err(CommandError::input("选区太小了，重新框一下"));
    }

    let dir = image_dir(&state);
    let result = tauri::async_runtime::spawn_blocking(move || -> Result<InlineImageDto, CommandError> {
        let image = xcap::image::DynamicImage::ImageRgba8(rgba_to_image(shot.width, shot.height, shot.rgba)?);
        let left = x.min(image.width().saturating_sub(1));
        let top = y.min(image.height().saturating_sub(1));
        let region_width = width.min(image.width().saturating_sub(left)).max(1);
        let region_height = height.min(image.height().saturating_sub(top)).max(1);
        let cropped = image.crop_imm(left, top, region_width, region_height);

        let mut buffer = Vec::new();
        cropped
            .write_to(&mut Cursor::new(&mut buffer), xcap::image::ImageFormat::Png)
            .map_err(|error| CommandError::new(format!("编码截图失败：{error}")))?;
        std::fs::create_dir_all(&dir)
            .map_err(|error| CommandError::new(format!("建图片目录失败：{error}")))?;
        let filename = next_filename("截图", "png");
        let path = dir.join(&filename);
        std::fs::write(&path, &buffer)
            .map_err(|error| CommandError::new(format!("保存截图失败：{error}")))?;
        to_dto(path, filename, buffer)
    })
    .await
    .map_err(|error| CommandError::new(format!("截图任务失败：{error}")));

    // 无论成败都把界面还回来，不能停在「什么都看不见」的状态。
    let _ = close_overlay_and_restore(&app);

    let dto = result??;
    app.emit_to("main", SCREENSHOT_EVENT, dto)
        .map_err(|error| CommandError::new(format!("把截图交给写信窗格失败：{error}")))?;
    Ok(())
}

/// 取消截图：丢掉待裁图，关掉叠加窗，把主窗口还回来。
#[tauri::command]
pub async fn cancel_screenshot(app: AppHandle) -> Result<(), CommandError> {
    clear_pending_shot();
    let _ = close_overlay_and_restore(&app);
    Ok(())
}

/// 把 RGBA 原始字节组装成图像；尺寸对不上时给可读错误，不让它 panic。
fn rgba_to_image(width: u32, height: u32, rgba: Vec<u8>) -> Result<xcap::image::RgbaImage, CommandError> {
    let expected = (width as usize) * (height as usize) * 4;
    if width == 0 || height == 0 || rgba.len() < expected {
        return Err(CommandError::new("截图数据不完整，请重新截一次"));
    }
    xcap::image::RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| CommandError::new("截图数据不完整，请重新截一次"))
}

/// 丢掉待裁的整屏图。
fn clear_pending_shot() {
    let mut guard = PENDING_SHOT
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = None;
}

/// 关掉叠加窗并把主窗口叫回来。
fn close_overlay_and_restore(app: &AppHandle) -> Result<(), CommandError> {
    if let Some(overlay) = app.get_webview_window(SCREENSHOT_WINDOW) {
        let _ = overlay.close();
    }
    show_main_window(app);
    Ok(())
}

/// 显示主窗口并置前；截图流程里失败也要走到这里。
fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
