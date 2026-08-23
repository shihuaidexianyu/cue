//! 行图标(§118):文件夹 / 通用文件各一张,load 时后台线程一次性
//! 提取。`SHGFI_USEFILEATTRIBUTES` 让 SHGetFileInfoW 只按属性给系统
//! 图标,不触盘;不做按扩展名的真实类型图标(V1.x 再议——要嘛逐行
//! 提取上 UI 线程不行,要嘛按扩展名缓存,属于增量优化不是必须)。
//!
//! `hicon_to_rgba` / `normalize_bbox` 复制自 cue-module-bookmark
//! (Rule of Three 第三次;util 下沉单独提交)。

use cue_protocol::IconImage;
use std::sync::Arc;
use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL};
use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
use windows::Win32::UI::Shell::{
    SHGetFileInfoW, SHGetImageList, SHFILEINFOW, SHGFI_SYSICONINDEX, SHGFI_USEFILEATTRIBUTES,
    SHIL_JUMBO,
};
use windows::Win32::UI::WindowsAndMessaging::*;

/// §14:单尺寸 96px。
const ICON_SIZE: u32 = 96;

pub struct FileIcons {
    pub folder: Arc<IconImage>,
    pub file: Arc<IconImage>,
}

/// 提取两张系统图标。调用方负责在 COM 线程上调用(ComGuard;
/// SHGetImageList 需要)。任一失败返回 None(present 走 SystemIcon 兜底)。
pub fn load_file_icons() -> Option<FileIcons> {
    let folder = extract_system_icon("folder", FILE_ATTRIBUTE_DIRECTORY)?;
    let file = extract_system_icon("file", FILE_ATTRIBUTE_NORMAL)?;
    Some(FileIcons {
        folder: Arc::new(folder),
        file: Arc::new(file),
    })
}

fn extract_system_icon(
    fake_name: &str,
    attrs: windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES,
) -> Option<IconImage> {
    unsafe {
        let wide: Vec<u16> = fake_name.encode_utf16().chain(Some(0)).collect();
        let mut shfi = SHFILEINFOW::default();
        let got = SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            attrs,
            Some(&mut shfi),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_SYSICONINDEX | SHGFI_USEFILEATTRIBUTES,
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
                biHeight: -(size as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
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

/// 老式图标的 JUMBO 单元是"原尺寸贴左上角的 256px 大图",用 alpha
/// 包围盒检测并裁出放大居中;正常图标不动(同 bookmark 注释)。
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
        return;
    }
    let (bw, bh) = (max_x - min_x + 1, max_y - min_y + 1);
    if bw * 2 >= s && bh * 2 >= s {
        return;
    }
    let scale = (s as f32 / bw as f32).min(s as f32 / bh as f32);
    let (dw, dh) = ((bw as f32 * scale) as usize, (bh as f32 * scale) as usize);
    let (ox, oy) = ((s - dw) / 2, (s - dh) / 2);
    let src = rgba.clone();
    let mut dst = vec![0u8; s * s * 4];
    for dy in 0..dh {
        for dx in 0..dw {
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

    /// 系统文件夹图标任何 Windows 都提得出;验证管线与像素契约。
    #[test]
    fn folder_icon_extracts() {
        let _com = crate::com::ComGuard::new();
        let icons = load_file_icons().expect("system folder/file icons");
        for icon in [icons.folder, icons.file] {
            assert_eq!((icon.width, icon.height), (ICON_SIZE, ICON_SIZE));
            assert_eq!(icon.rgba.len() as u32, ICON_SIZE * ICON_SIZE * 4);
            assert!(
                icon.rgba.chunks_exact(4).any(|px| px[3] > 0),
                "图标不应全透明"
            );
        }
    }
}
