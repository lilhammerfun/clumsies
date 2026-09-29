//! The Windows client's own resource: the application icon.
//!
//! Explorer, the taskbar and every shortcut take a Windows program's icon from a
//! resource inside the executable, so the client carries the same mark the
//! package ships as a file. Nothing else needs a build script: on Linux the icon
//! comes from the `.desktop` entry and the hicolor theme.
//!
//! A resource compiler that is missing or unhappy is a warning rather than a
//! failure: an icon is not worth failing a release over, and CI shows the
//! warning in the log where it can be seen.

fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/icons/clumsies.ico");
        let mut resource = winres::WindowsResource::new();
        resource.set_icon("assets/icons/clumsies.ico");
        resource.set("FileDescription", "Clumsies");
        resource.set("ProductName", "Clumsies");
        resource.set("OriginalFilename", "clumsies-desktop.exe");
        if let Err(error) = resource.compile() {
            println!("cargo:warning=could not embed the Windows icon: {error}");
        }
    }
}
