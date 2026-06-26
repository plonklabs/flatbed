fn main() {
    println!("cargo:rerun-if-env-changed=FLATBED_GENERATE");

    if std::env::var("FLATBED_GENERATE").as_deref() == Ok("1") {
        flatbed_build::Config::new()
            .schema("schemas/test.fbs")
            .compile()
            .expect("flatbed_build compilation failed");
    }
}
