use super::super::i18n::Language;
use libloading::Library;
use std::{
    ffi::{CStr, CString, c_char, c_void},
    os::unix::ffi::OsStrExt,
    path::PathBuf,
    process::Command,
    ptr,
    sync::OnceLock,
};
type Widget = *mut c_void;

static GTK: OnceLock<Result<Library, String>> = OnceLock::new();

fn gtk() -> Result<&'static Library, String> {
    // Load the desktop component only when requested, without a GTK link-time
    // dependency. Missing GTK or a missing display leaves manual paths usable.
    unsafe {
        let library = GTK
            .get_or_init(|| {
                // GtkBuilder looks up GTK/Pango type constructors in the
                // process namespace, so RTLD_LOCAL breaks its built-in UIs.
                libloading::os::unix::Library::open(
                    Some("libgtk-3.so.0"),
                    libloading::os::unix::RTLD_NOW | libloading::os::unix::RTLD_GLOBAL,
                )
                .map(Library::from)
                .map_err(|_| {
                    "GTK 3 font/file dialogs are unavailable; enter a font file path instead."
                        .to_owned()
                })
            })
            .as_ref()
            .map_err(Clone::clone)?;
        let init = library
            .get::<unsafe extern "C" fn(*mut i32, *mut *mut *mut c_char) -> i32>(
                b"gtk_init_check\0",
            )
            .map_err(|e| e.to_string())?;
        if init(ptr::null_mut(), ptr::null_mut()) == 0 {
            return Err("No graphical desktop is available for the font dialog.".into());
        }
        Ok(library)
    }
}

pub(crate) fn choose_family(current: &str, language: Language) -> Result<Option<String>, String> {
    let initial = CString::new(format!(
        "{} 18",
        if current.is_empty() { "Sans" } else { current }
    ))
    .map_err(|e| e.to_string())?;
    let library = gtk()?;
    // Signatures follow GTK 3/Pango's stable C ABI. Resolve everything before
    // allocating a dialog, and copy borrowed text before freeing its owner.
    unsafe {
        let create = library
            .get::<unsafe extern "C" fn(*const c_char, Widget) -> Widget>(
                b"gtk_font_chooser_dialog_new\0",
            )
            .map_err(|e| e.to_string())?;
        let set = library
            .get::<unsafe extern "C" fn(Widget, *const c_char)>(b"gtk_font_chooser_set_font\0")
            .map_err(|e| e.to_string())?;
        let get = library
            .get::<unsafe extern "C" fn(Widget) -> *mut c_char>(b"gtk_font_chooser_get_font\0")
            .map_err(|e| e.to_string())?;
        let run = library
            .get::<unsafe extern "C" fn(Widget) -> i32>(b"gtk_dialog_run\0")
            .map_err(|e| e.to_string())?;
        let destroy = library
            .get::<unsafe extern "C" fn(Widget)>(b"gtk_widget_destroy\0")
            .map_err(|e| e.to_string())?;
        let parse = library
            .get::<unsafe extern "C" fn(*const c_char) -> Widget>(
                b"pango_font_description_from_string\0",
            )
            .map_err(|e| e.to_string())?;
        let family = library
            .get::<unsafe extern "C" fn(Widget) -> *const c_char>(
                b"pango_font_description_get_family\0",
            )
            .map_err(|e| e.to_string())?;
        let free_description = library
            .get::<unsafe extern "C" fn(Widget)>(b"pango_font_description_free\0")
            .map_err(|e| e.to_string())?;
        let free = library
            .get::<unsafe extern "C" fn(Widget)>(b"g_free\0")
            .map_err(|e| e.to_string())?;
        let flush = library
            .get::<unsafe extern "C" fn()>(b"gdk_flush\0")
            .map_err(|e| e.to_string())?;
        let title = CString::new(language.text("ui.choose_font")).unwrap();
        let dialog = create(title.as_ptr(), ptr::null_mut());
        if dialog.is_null() {
            return Err("Could not create the font dialog".into());
        }
        set(dialog, initial.as_ptr());
        if let Ok(level) =
            library.get::<unsafe extern "C" fn(Widget, u32)>(b"gtk_font_chooser_set_level\0")
        {
            level(dialog, 0);
        }
        let response = run(dialog);
        let mut selected = None;
        if response == -5 || response == -3 {
            let text = get(dialog);
            if !text.is_null() {
                let description = parse(text);
                if !description.is_null() {
                    let name = family(description);
                    if !name.is_null() {
                        selected = Some(CStr::from_ptr(name).to_string_lossy().into_owned());
                    }
                    free_description(description);
                }
                free(text.cast());
            }
        }
        destroy(dialog);
        flush();
        Ok(selected)
    }
}

pub(crate) fn choose_file(language: Language) -> Result<Option<PathBuf>, String> {
    let library = gtk()?;
    unsafe {
        let create = library
            .get::<unsafe extern "C" fn(*const c_char, Widget, i32, *const c_char, ...) -> Widget>(
                b"gtk_file_chooser_dialog_new\0",
            )
            .map_err(|e| e.to_string())?;
        let run = library
            .get::<unsafe extern "C" fn(Widget) -> i32>(b"gtk_dialog_run\0")
            .map_err(|e| e.to_string())?;
        let get = library
            .get::<unsafe extern "C" fn(Widget) -> *mut c_char>(b"gtk_file_chooser_get_filename\0")
            .map_err(|e| e.to_string())?;
        let destroy = library
            .get::<unsafe extern "C" fn(Widget)>(b"gtk_widget_destroy\0")
            .map_err(|e| e.to_string())?;
        let free = library
            .get::<unsafe extern "C" fn(Widget)>(b"g_free\0")
            .map_err(|e| e.to_string())?;
        let flush = library
            .get::<unsafe extern "C" fn()>(b"gdk_flush\0")
            .map_err(|e| e.to_string())?;
        let title = CString::new(language.text("ui.choose_font_file_ttf_otf_ttc")).unwrap();
        let cancel = CString::new(language.text("ui.cancel")).unwrap();
        let open = CString::new(language.text("ui.open")).unwrap();
        let dialog = create(
            title.as_ptr(),
            ptr::null_mut(),
            0,
            cancel.as_ptr(),
            -6i32,
            open.as_ptr(),
            -3i32,
            ptr::null::<c_char>(),
        );
        if dialog.is_null() {
            return Err("Could not create the file dialog".into());
        }
        let response = run(dialog);
        let mut selected = None;
        if response == -3 {
            let filename = get(dialog);
            if !filename.is_null() {
                selected = Some(PathBuf::from(std::ffi::OsStr::from_bytes(
                    CStr::from_ptr(filename).to_bytes(),
                )));
                free(filename.cast());
            }
        }
        destroy(dialog);
        flush();
        Ok(selected)
    }
}

pub(crate) fn font_bytes(family: &str) -> Result<Vec<u8>, String> {
    let output = Command::new("fc-match")
        .args(["--format=%{file}", "--", family])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() || output.stdout.is_empty() {
        return Err("Could not locate the selected system font".into());
    }
    let path = PathBuf::from(std::ffi::OsStr::from_bytes(&output.stdout));
    if path.metadata().map_err(|e| e.to_string())?.len() > 64 * 1024 * 1024 {
        return Err("Font file exceeds 64 MiB".into());
    }
    std::fs::read(path).map_err(|e| e.to_string())
}
