#[cfg(windows)]
fn main() {
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/irongrp.ico");
    res.compile().expect("failed to compile Windows resources");
}

#[cfg(not(windows))]
fn main() {}
