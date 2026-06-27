fn main() {
    flatbed_build::Config::new()
        .schema("schemas/info.fbs")
        .compile()
        .expect("flatbed codegen failed");
}
