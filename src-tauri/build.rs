fn main() {
    println!("cargo:rerun-if-changed=icons/icon.ico");
    println!("cargo:rerun-if-changed=icons/32x32.png");
    println!("cargo:rerun-if-changed=icons/128x128.png");
    println!("cargo:rerun-if-changed=icons/128x128@2x.png");
    println!("cargo:rerun-if-changed=icons/icon.png");
    tauri_build::build();

    // 把 webview2-com-sys 生成的 WebView2Loader.dll 拷到 src-tauri/bin/，让 NSIS
    // bundler 通过 bundle.resources 把 dll 装到与 exe 同目录。
    //
    // MSVC 工具链下 webview2-com-sys 走静态链接，runtime 不需要 dll，但 build.rs
    // 仍会执行 webview2_link::output_libs 把 dll 与 .lib 一并拷到 OUT_DIR ——
    // 我们这里把 dll 也复制出去，bundle 里多 160KB 但对运行时无影响。
    let out_dir = match std::env::var_os("OUT_DIR") {
        Some(v) => std::path::PathBuf::from(v),
        None => return,
    };
    let manifest_dir = match std::env::var_os("CARGO_MANIFEST_DIR") {
        Some(v) => std::path::PathBuf::from(v),
        None => return,
    };
    let bin_dir = manifest_dir.join("bin");
    if let Err(e) = std::fs::create_dir_all(&bin_dir) {
        eprintln!("cargo:warning=create bin/ failed: {e}");
        return;
    }

    // webview2-com-sys 在 OUT_DIR 下按 target_arch 分目录放 dll：x64 / x86 / arm64。
    let target_arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let arch_dir = match target_arch.as_str() {
        "x86_64" => "x64",
        "x86" => "x86",
        "aarch64" => "arm64",
        other => {
            eprintln!("cargo:warning=unsupported target arch: {other}");
            return;
        }
    };
    let src = out_dir.join(arch_dir).join("WebView2Loader.dll");
    let dst = bin_dir.join("WebView2Loader.dll");
    if !src.exists() {
        return;
    }
    match std::fs::copy(&src, &dst) {
        Ok(_) => println!("cargo:rerun-if-changed={}", src.display()),
        Err(e) => eprintln!("cargo:warning=copy WebView2Loader.dll failed: {e}"),
    }
}
