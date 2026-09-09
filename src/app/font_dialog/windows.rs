//! Windows common dialogs and outline data for an installed font family.
use super::super::i18n::Language;
use std::{ffi::OsString, os::windows::ffi::OsStringExt, path::PathBuf, ptr};
use windows_sys::Win32::{
    Graphics::Gdi::*,
    UI::{Controls::Dialogs::*, Input::KeyboardAndMouse::GetActiveWindow},
};

fn logfont(family: &str) -> Result<LOGFONTW, String> {
    let name: Vec<u16> = family.encode_utf16().collect();
    let mut font = LOGFONTW {
        lfHeight: -18,
        lfWeight: FW_NORMAL as i32,
        lfCharSet: DEFAULT_CHARSET,
        ..Default::default()
    };
    if name.len() >= font.lfFaceName.len() {
        return Err("Font family name is too long".into());
    }
    font.lfFaceName[..name.len()].copy_from_slice(&name);
    Ok(font)
}

fn dialog_result<T>(success: bool, result: T) -> Result<Option<T>, String> {
    if success {
        return Ok(Some(result));
    }
    // No pointers are involved; zero means the user cancelled the dialog.
    let code = unsafe { CommDlgExtendedError() };
    if code == 0 {
        Ok(None)
    } else {
        Err(format!("Windows dialog failed: {code:#x}"))
    }
}

pub(crate) fn choose_family(current: &str, _language: Language) -> Result<Option<String>, String> {
    let mut font = logfont(current)?;
    // Buffers remain alive for the entire synchronous modal call. The active
    // window belongs to this UI thread and owns the dialog.
    let mut chooser = CHOOSEFONTW {
        lStructSize: std::mem::size_of::<CHOOSEFONTW>() as u32,
        hwndOwner: unsafe { GetActiveWindow() },
        lpLogFont: &mut font,
        Flags: CF_SCREENFONTS
            | CF_TTONLY
            | CF_INITTOLOGFONTSTRUCT
            | CF_NOSTYLESEL
            | CF_NOSIZESEL
            | CF_NOSCRIPTSEL
            | CF_NOVERTFONTS,
        ..Default::default()
    };
    let success = unsafe { ChooseFontW(&mut chooser) } != 0;
    let length = font
        .lfFaceName
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(font.lfFaceName.len());
    dialog_result(
        success,
        String::from_utf16_lossy(&font.lfFaceName[..length]),
    )
}

pub(crate) fn choose_file(language: Language) -> Result<Option<PathBuf>, String> {
    let filter: Vec<u16> = language
        .text("Fonts (*.ttf;*.otf;*.ttc)")
        .encode_utf16()
        .chain("\0*.ttf;*.otf;*.ttc\0\0".encode_utf16())
        .collect();
    let title: Vec<u16> = language
        .text("Choose font file")
        .encode_utf16()
        .chain([0])
        .collect();
    let mut buffer = vec![0u16; 32768];
    let mut chooser = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        hwndOwner: unsafe { GetActiveWindow() },
        lpstrFilter: filter.as_ptr(),
        lpstrTitle: title.as_ptr(),
        lpstrFile: buffer.as_mut_ptr(),
        nMaxFile: buffer.len() as u32,
        Flags: OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR | OFN_EXPLORER,
        ..Default::default()
    };
    // All UTF-16 strings and the writable result buffer outlive the call.
    let success = unsafe { GetOpenFileNameW(&mut chooser) } != 0;
    let length = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    dialog_result(
        success,
        PathBuf::from(OsString::from_wide(&buffer[..length])),
    )
}

pub(crate) fn font_bytes(family: &str) -> Result<Vec<u8>, String> {
    let description = logfont(family)?;
    // Own the DC and font, restore the previously selected object before
    // deleting them, and bound the allocation before writing font bytes.
    unsafe {
        let dc = CreateCompatibleDC(ptr::null_mut());
        if dc.is_null() {
            return Err("Could not create a font device context".into());
        }
        let font = CreateFontIndirectW(&description);
        if font.is_null() {
            DeleteDC(dc);
            return Err("Could not open the system font".into());
        }
        let previous = SelectObject(dc, font);
        let result = (|| {
            if previous.is_null() {
                return Err("Could not select the system font".into());
            }
            super::font_data::read_font_data(|table, buffer| match buffer {
                None => GetFontData(dc, table, 0, ptr::null_mut(), 0),
                Some(bytes) => {
                    GetFontData(dc, table, 0, bytes.as_mut_ptr().cast(), bytes.len() as u32)
                }
            })
        })();
        if !previous.is_null() {
            SelectObject(dc, previous);
        }
        DeleteObject(font);
        DeleteDC(dc);
        if let Ok(bytes) = &result {
            let format = match bytes.get(..4) {
                Some(b"ttcf") => "TTC",
                Some(b"OTTO") => "OTF",
                _ => "TTF/other",
            };
            crate::diagnostics::record(format_args!(
                "system-font format={format} bytes={} faces={}",
                bytes.len(),
                ttf_parser::fonts_in_collection(bytes).unwrap_or(1)
            ));
        }
        result
    }
}
