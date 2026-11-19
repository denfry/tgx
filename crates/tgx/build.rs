fn main() {
    tdlib_rs::build::build(None);
    slint_build::compile("ui/app.slint").expect("compile ui/app.slint");
    // api_id/api_hash are baked in at build time when set (release CI); a dev
    // build falls back to the same variables at run time.
    println!("cargo:rerun-if-env-changed=TGX_API_ID");
    println!("cargo:rerun-if-env-changed=TGX_API_HASH");
}
