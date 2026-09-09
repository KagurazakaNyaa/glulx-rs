fn main() {
    println!(
        "cargo:rustc-env=GLULX_BUILD_OPT_LEVEL={}",
        std::env::var("OPT_LEVEL").unwrap()
    );
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/icon.ico")
            .compile()
            .expect("failed to embed the Windows application icon");
    }
}
