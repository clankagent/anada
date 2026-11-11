//! Embeds the application icon and the name that Windows shows for both executables.
fn main() {
    println!("cargo:rerun-if-changed=../../assets/anada.ico");
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("../../assets/anada.ico");
        resource.set("ProductName", "Anada");
        resource.set("FileDescription", "Anada");
        resource.compile().expect("Windows resources");
    }
}
