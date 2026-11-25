fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/vaultpack.slint", config).expect("Slint UI compilation failed");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/vaultpack.ico");
        res.set("ProductName", "Vaultpack");
        res.set("FileDescription", "Vaultpack");
        if let Err(e) = res.compile() {
            println!("cargo:warning=icon embedding skipped: {}", e);
        }
    }
}
