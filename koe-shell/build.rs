fn main() {
    // Embed app icon into the Windows executable
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/koe.ico");
        res.compile().expect("failed to compile Windows resources");
    }
}
