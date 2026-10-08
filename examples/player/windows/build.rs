//! Embeds the icon and version in the Windows executable.

fn main() {
    println!("cargo:rerun-if-changed=assets/sf2player.ico");
    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/sf2player.ico");
        resource.set("ProductName", "SF2 Player");
        resource.set("FileDescription", "SF2 Player");
        if let Err(error) = resource.compile() {
            println!("cargo:warning=could not embed the icon: {error}");
        }
    }
}
