fn main() {
    #[cfg(target_os = "windows")]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("chkn-logo.ico");
        res.compile().unwrap();
    }
}