//! 字体字形 → 96px RGBA 位图(§135):Segoe Fluent Icons / Segoe MDL2
//! Assets 的单色图标字形——Windows 11 原生图标语言(电源菜单、设置
//! 同源),取代 emoji 兜底。GDI 灰阶抗锯齿渲染到 32bpp DIB,亮度通道
//! 转 straight alpha × 指定颜色。
//!
//! 只依赖 GDI,不需要 COM 套间;渲染一次约亚毫秒,调用方自行缓存。

use cue_protocol::IconImage;
use std::sync::Arc;
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Gdi::*;
use windows::core::PCWSTR;

/// 按序回退的图标字体族。Fluent 是 Win11 原生;MDL2 是 Win10 起
/// 预装的同码位前身——同一份 codepoint 表两族通用。
const FAMILIES: [&str; 2] = ["Segoe Fluent Icons", "Segoe MDL2 Assets"];

/// wingdi.h GGI_MARK_NONEXISTING_GLYPHS:不存在的字形返回 0xFFFF。
const GGI_MARK_NONEXISTING_GLYPHS: u32 = 0x10;

/// 兜底字形的默认前景色:与行标题文字同档的浅灰——深色底上
/// 对比足够,又不抢彩色应用图标的视觉权重。
pub const DEFAULT_RGB: [u8; 3] = [0xE6, 0xE6, 0xE6];

/// 带缓存的字形渲染(§72 Rule of Three 下沉:app/file/bookmark
/// 三处兜底同形)。同一 (codepoint, rgb) 永远返回同一份 Arc
/// 像素缓冲——UI 按 Arc 指针缓存纹理;字体缺失的 None 也缓存,
/// 不在 present() 热路径上反复重试。
pub fn cached_glyph(codepoint: u32, rgb: [u8; 3]) -> Option<IconImage> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    type Cache = Mutex<HashMap<(u32, [u8; 3]), Option<IconImage>>>;
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    cache
        .lock()
        .unwrap()
        .entry((codepoint, rgb))
        .or_insert_with(|| render_glyph(codepoint, rgb))
        .clone()
}

/// 渲染一个 PUA 字形为 96×96 RGBA(straight alpha 契约)。
/// 两族字体都没有该字形时返回 None(调用方决定兜底)。
pub fn render_glyph(codepoint: u32, rgb: [u8; 3]) -> Option<IconImage> {
    let size = crate::icon::ICON_SIZE;
    let ch = char::from_u32(codepoint)?;
    let mut units_buf = [0u16; 2];
    let units: &[u16] = ch.encode_utf16(&mut units_buf);

    unsafe {
        let hdc = CreateCompatibleDC(None);
        if hdc.is_invalid() {
            return None;
        }
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32), // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let dib = match CreateDIBSection(Some(hdc), &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(dib) if !bits.is_null() => dib,
            Ok(dib) => {
                let _ = DeleteObject(dib.into());
                let _ = DeleteDC(hdc);
                return None;
            }
            Err(_) => {
                let _ = DeleteDC(hdc);
                return None;
            }
        };
        std::ptr::write_bytes(bits as *mut u8, 0, (size * size * 4) as usize);
        let old_bmp = SelectObject(hdc, dib.into());

        let mut result = None;
        'families: for family in FAMILIES {
            let face: Vec<u16> = family.encode_utf16().chain(Some(0)).collect();
            // em 高度取画布 2/3:MDL2/Fluent 字形的油墨区 ≈ em 的 70%,
            // 96px 画布上墨迹约 45px,行内 32px 槽位降采样后与 exe
            // 图标视觉重量相当。
            let font = CreateFontW(
                -((size as i32) * 2 / 3),
                0,
                0,
                0,
                400, // FW_NORMAL
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                ANTIALIASED_QUALITY, // 灰阶 AA;ClearType 会留彩色边缘
                0,
                PCWSTR(face.as_ptr()),
            );
            if font.is_invalid() {
                continue;
            }
            let old_font = SelectObject(hdc, font.into());

            // 字形存在性:族内缺失别画 .notdef 豆腐块,换下一族。
            let mut glyph_index = [0u16; 2];
            GetGlyphIndicesW(
                hdc,
                PCWSTR(units.as_ptr()),
                units.len() as i32,
                glyph_index.as_mut_ptr(),
                GGI_MARK_NONEXISTING_GLYPHS,
            );
            if glyph_index[0] != 0xFFFF {
                let mut extent = windows::Win32::Foundation::SIZE::default();
                let _ = GetTextExtentPoint32W(hdc, units, &mut extent);
                SetBkMode(hdc, TRANSPARENT);
                SetTextColor(hdc, COLORREF(0x00FF_FFFF)); // 白字黑底:亮度=覆盖率
                let x = ((size as i32) - extent.cx).max(0) / 2;
                let y = ((size as i32) - extent.cy).max(0) / 2;
                let drawn = ExtTextOutW(
                    hdc,
                    x,
                    y,
                    ETO_OPTIONS(0),
                    None,
                    PCWSTR(units.as_ptr()),
                    units.len() as u32,
                    None,
                );
                if drawn.as_bool() {
                    let raw =
                        std::slice::from_raw_parts(bits as *const u8, (size * size * 4) as usize);
                    let mut out = vec![0u8; (size * size * 4) as usize];
                    for (dst, src) in out
                        .as_chunks_mut::<4>()
                        .0
                        .iter_mut()
                        .zip(raw.as_chunks::<4>().0)
                    {
                        // BGRA:灰阶 AA 下三通道同值,取其一作覆盖率。
                        let coverage = src[2];
                        dst[0] = rgb[0];
                        dst[1] = rgb[1];
                        dst[2] = rgb[2];
                        dst[3] = coverage;
                    }
                    result = Some(IconImage::new(Arc::from(out), size, size));
                }
            }

            SelectObject(hdc, old_font);
            let _ = DeleteObject(font.into());
            if result.is_some() {
                break 'families;
            }
        }

        SelectObject(hdc, old_bmp);
        let _ = DeleteObject(dib.into());
        let _ = DeleteDC(hdc);
        result
    }
}

#[cfg(test)]
mod tests {
    /// 候选字形验收表(诊断工具,非常规测试):把候选 codepoint 渲染成
    /// 5 列 contact sheet,肉眼挑选定案。
    /// 运行:cargo test -p cue-util-win glyph_sheet -- --ignored --nocapture
    /// 产物:target/glyph-audit/sheet.png(行序 = 候选表序)
    #[test]
    #[ignore]
    fn glyph_sheet() {
        let candidates: [(&str, u32); 20] = [
            ("snow E9C8", 0xE9C8),
            ("frigid E9CA", 0xE9CA),
            ("cand E9CB", 0xE9CB),
            ("cand E9CC", 0xE9CC),
            ("cand E9D4", 0xE9D4),
            ("cand E94F", 0xE94F),
            ("cand E7BA", 0xE7BA),
            ("cand E80A", 0xE80A),
            ("cand E9A6", 0xE9A6),
            ("cand E823", 0xE823),
            ("lock E72E", 0xE72E),
            ("moon E708", 0xE708),
            ("power E7E8", 0xE7E8),
            ("refresh E72C", 0xE72C),
            ("signout F3B1", 0xF3B1),
            ("delete E74D", 0xE74D),
            ("appicon ECAA", 0xECAA),
            ("folder E8B7", 0xE8B7),
            ("page E8A5", 0xE8A5),
            ("globe E774", 0xE774),
        ];
        const CELL: u32 = crate::icon::ICON_SIZE;
        const COLS: u32 = 5;
        let rows = (candidates.len() as u32).div_ceil(COLS);
        let mut sheet = image::RgbaImage::new(COLS * CELL, rows * CELL);
        for (i, (label, cp)) in candidates.iter().enumerate() {
            let icon = super::render_glyph(*cp, [0xE6, 0xE6, 0xE6])
                .unwrap_or_else(|| panic!("glyph missing: {label}"));
            let (gx, gy) = (i as u32 % COLS, i as u32 / COLS);
            for y in 0..CELL {
                for x in 0..CELL {
                    let s = ((y * CELL + x) * 4) as usize;
                    sheet.put_pixel(
                        gx * CELL + x,
                        gy * CELL + y,
                        image::Rgba([
                            icon.rgba[s],
                            icon.rgba[s + 1],
                            icon.rgba[s + 2],
                            icon.rgba[s + 3],
                        ]),
                    );
                }
            }
        }
        let out_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/glyph-audit");
        std::fs::create_dir_all(&out_dir).unwrap();
        sheet.save(out_dir.join("sheet.png")).unwrap();
        for (i, (label, _)) in candidates.iter().enumerate() {
            println!("cell {} (col {}, row {}): {label}", i, i % 5, i / 5);
        }
    }
}
