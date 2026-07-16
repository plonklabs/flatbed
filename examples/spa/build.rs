fn main() {
    flatbed_build::Config::new()
        .schema("schemas/api.fbs")
        .compile()
        .expect("flatbed codegen failed");
}
