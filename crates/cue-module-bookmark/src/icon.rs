//! 行图标(§117):每行显示来源浏览器的图标(Edge/Chrome exe 提取),
//! 不做网站 favicon(Flow 默认关;要读 "Favicons" sqlite + 拷贝解锁,
//! 复杂度与依赖不值)。提取失败/浏览器未装 → SystemIcon::Generic。
//!
//! `extract_icon` / `hicon_to_rgba` / `normalize_bbox` 复制自
//! cue-module-app(Rule of Three 第二次使用,§72;第三个消费者落地
//! 时下沉 util crate)。与 app 侧的差异只在调用形态:浏览器图标仅
//! 2 张,load 时在模块后台线程同步提取一次,不需要 worker 管线。

use crate::chromium::Browser;
use cue_protocol::IconImage;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
use windows::Win32::UI::Shell::{
    SHGetFileInfoW, SHGetImageList, SHFILEINFOW, SHGFI_SYSICONINDEX, SHIL_JUMBO,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// §14:单尺寸 96px。
const ICON_SIZE: u32 = 96;

/// 每个已发现浏览器提取一张 exe 图标。调用方负责在 COM 线程上调用
/// (ComGuard;SHGetImageList 需要)。找不到 exe / 提取失败的浏览器
/// 不进 map(present 走 SystemIcon 兜底)。
pub fn load_browser_icons() -> HashMap<Browser, Arc<IconImage>> {
    let mut out = HashMap::new();
    for browser in [Browser::Edge, Browser::Chrome] {
        if let Some(icon) = browser.exe_path().and_then(|exe| extract_icon(&exe)) {
            out.insert(browser, Arc::new(icon));
        }
    }
    out
}

/// 提取 exe 的 256px 系统图标并降采样到 96px RGBA。
fn extract_icon(exe: &Path) -> Option<IconImage> {
    unsafe {
        let wide: Vec<u16> = exe
            .to_string_lossy()
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut shfi = SHFILEINFOW::default();
        let got = SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut shfi),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX,
        );
        if got == 0 {
            return None;
        }
        let list: IImageList = SHGetImageList(SHIL_JUMBO as i32).ok()?;
        let hicon = list.GetIcon(shfi.iIcon, ILD_TRANSPARENT.0).ok()?;
        let icon = hicon_to_rgba(hicon, ICON_SIZE);
        let _ = DestroyIcon(hicon);
        icon
    }
}

/// HICON → 96×96 RGBA(straight alpha,§14 像素契约)。
/// GDI 32bpp DIB 为 BGRA,逐像素交换 R/B。
fn hicon_to_rgba(hicon: HICON, size: u32) -> Option<IconImage> {
    unsafe {
        let hdc = CreateCompatibleDC(None);
        if hdc.is_invalid() {
            return None;
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32), // top-down,读像素不用倒序
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        // 失败路径必须先 DeleteDC——CreateCompatibleDC 已成功,
        // 直接 `?` 返回会泄漏这个 GDI DC。
        let dib = match CreateDIBSection(Some(hdc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(dib) => dib,
            Err(_) => {
                let _ = DeleteDC(hdc);
                return None;
            }
        };
        if bits.is_null() {
            let _ = DeleteObject(dib.into());
            let _ = DeleteDC(hdc);
            return None;
        }
        std::ptr::write_bytes(bits as *mut u8, 0, (size * size * 4) as usize);
        let old = SelectObject(hdc, dib.into());
        let drawn = DrawIconEx(
            hdc,
            0,
            0,
            hicon,
            size as i32,
            size as i32,
            0,
            None,
            DI_NORMAL,
        );
        let mut rgba = None;
        if drawn.is_ok() {
            let raw =
                std::slice::from_raw_parts(bits as *const u8, (size * size * 4) as usize).to_vec();
            let mut out = raw;
            for px in out.chunks_exact_mut(4) {
                px.swap(0, 2); // BGRA → RGBA
            }
            normalize_bbox(&mut out, size);
            rgba = Some(IconImage::new(Arc::from(out), size, size));
        }
        SelectObject(hdc, old);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(hdc);
        rgba
    }
}

/// 老式应用的 JUMBO image list 单元是"原尺寸贴在左上角的 256px
/// 大图"(shell 不上采样),直接缩放会得到缩在角落的小图标。
/// 用 alpha 包围盒检测这种异常:内容宽或高不足画布一半时,
/// 裁出内容双线性放大并居中。正常图标(包围盒 ≈ 全画布)不动,
/// 避免把自带留边的图标放大得大小不一。
fn normalize_bbox(rgba: &mut Vec<u8>, size: u32) {
    let s = size as usize;
    let (mut min_x, mut min_y, mut max_x, mut max_y) = (s, s, 0usize, 0usize);
    for y in 0..s {
        for x in 0..s {
            if rgba[(y * s + x) * 4 + 3] > 16 {
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if max_x < min_x || max_y < min_y {
        return; // 全透明:无可归一化
    }
    let (bw, bh) = (max_x - min_x + 1, max_y - min_y + 1);
    if bw * 2 >= s && bh * 2 >= s {
        return; // 内容 ≥ 半画布:正常图标
    }
    // 等比放大到尽量填满画布,居中
    let scale = (s as f32 / bw as f32).min(s as f32 / bh as f32);
    let (dw, dh) = ((bw as f32 * scale) as usize, (bh as f32 * scale) as usize);
    let (ox, oy) = ((s - dw) / 2, (s - dh) / 2);
    let src = rgba.clone();
    let mut dst = vec![0u8; s * s * 4];
    for dy in 0..dh {
        for dx in 0..dw {
            // 目标像素中心映射回源内容坐标(双线性)
            let fx = (dx as f32 + 0.5) / scale - 0.5;
            let fy = (dy as f32 + 0.5) / scale - 0.5;
            let (x0, y0) = (fx.floor().max(0.0) as usize, fy.floor().max(0.0) as usize);
            let (x1, y1) = ((x0 + 1).min(bw - 1), (y0 + 1).min(bh - 1));
            let (tx, ty) = (fx.fract().max(0.0), fy.fract().max(0.0));
            let mut px = [0f32; 4];
            for (sy, wy) in [(y0, 1.0 - ty), (y1, ty)] {
                for (sx, wx) in [(x0, 1.0 - tx), (x1, tx)] {
                    let i = ((min_y + sy) * s + (min_x + sx)) * 4;
                    let w = wx * wy;
                    for c in 0..4 {
                        px[c] += src[i + c] as f32 * w;
                    }
                }
            }
            let d = ((oy + dy) * s + (ox + dx)) * 4;
            for c in 0..4 {
                dst[d + c] = px[c].round() as u8;
            }
        }
    }
    *rgba = dst;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 本机 Edge 必装(§117 目标平台现实),其 exe 图标必须提得出。
    #[test]
    fn edge_icon_extracts() {
        let _com = crate::com::ComGuard::new();
        let Some(exe) = Browser::Edge.exe_path() else {
            eprintln!("edge not installed, skipping");
            return;
        };
        let icon = extract_icon(&exe).expect("edge icon extraction");
        assert_eq!((icon.width, icon.height), (ICON_SIZE, ICON_SIZE));
        assert_eq!(icon.rgba.len() as u32, ICON_SIZE * ICON_SIZE * 4);
        assert!(
            icon.rgba.chunks_exact(4).any(|px| px[3] > 0),
            "图标不应全透明"
        );
    }
}
