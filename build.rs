//! Embarque l'icone et les metadonnees Windows dans l'executable.

fn main() {
    println!("cargo:rerun-if-changed=assets/ruche.ico");
    println!("cargo:rerun-if-changed=build.rs");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("assets/ruche.ico");
    resource.set("ProductName", "Ruche");
    resource.set("FileDescription", "Launcher Minecraft multi-comptes");
    resource.set("LegalCopyright", "MIT");
    // Sans compilateur de ressources, on prefere un exe sans icone a un build casse.
    if let Err(error) = resource.compile() {
        println!("cargo:warning=icone non embarquee : {error}");
    }
}
