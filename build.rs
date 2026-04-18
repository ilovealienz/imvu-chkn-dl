fn main() {
    let mut res = winresource::WindowsResource::new();
    res.set_icon("chkn-logo.ico");
    res.compile().unwrap();
}