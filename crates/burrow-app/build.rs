fn main() {
    #[cfg(target_os = "windows")]
    {
        let rc = "resources/windows/burrow.rc";
        println!("cargo:rerun-if-changed={rc}");
        println!("cargo:rerun-if-changed=resources/windows/burrow.manifest.xml");
        println!("cargo:rerun-if-changed=resources/windows/burrow.ico");
        embed_resource::compile(rc, embed_resource::NONE)
            .manifest_required()
            .unwrap();
    }
}
